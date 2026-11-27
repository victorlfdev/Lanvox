//! Screen sharing: listing screens and windows with thumbnails, and the capture
//! loop that sends a share at the chosen quality.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::codec::{fit_resolution, into_rgba, scale_rgba, PixelFormat, RateControl, TileEncoder};
use super::cursor::{self, Rect};
use super::video::PictureStore;
use crate::model::{StreamKind, UserId};
use crate::net::client::{ClientSender, KeyframeFlags};

/// A full picture is sent at least this often. Loss recovery is on demand
/// (receivers request one), so this is only a safety net: IDRs are the biggest,
/// most loss-prone messages of the stream.
pub const KEYFRAME_INTERVAL: Duration = Duration::from_secs(8);
/// Never answer keyframe requests faster than this; a request that crosses an
/// IDR already in flight must not trigger another huge burst.
const MIN_KEYFRAME_GAP: Duration = Duration::from_millis(600);
/// Bitrate ceiling, whatever the resolution (kbit/s).
const MAX_KBPS_LIMIT: u32 = 40_000;
/// Deepest adaptation step. Reduce resolution before reducing the requested FPS.
const MAX_LEVEL: u32 = 4;
/// Smallest side length the adaptation may scale down to.
const MIN_ADAPTED_SIDE: u32 = 480;
/// Default stream quality.
pub const DEFAULT_QUALITY: u8 = 100;
/// Thumbnail size in the picker.
const THUMB_W: u32 = 320;
const THUMB_H: u32 = 180;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceId {
    Monitor(u32),
    Window(u32),
}

#[derive(Clone)]
pub struct ShareSource {
    pub id: SourceId,
    pub name: String,
    pub app: String,
    pub width: u32,
    pub height: u32,
    pub thumb: Option<(u32, u32, Vec<u8>)>,
    pub process_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Resolution {
    P480,
    P720,
    P1080,
    P1440,
    Source,
}

impl Resolution {
    /// Resolution options offered in the picker.
    pub const ALL: [Resolution; 5] = [
        Resolution::P480,
        Resolution::P720,
        Resolution::P1080,
        Resolution::P1440,
        Resolution::Source,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Resolution::P480 => "480p",
            Resolution::P720 => "720p",
            Resolution::P1080 => "1080p",
            Resolution::P1440 => "1440p",
            Resolution::Source => "Original",
        }
    }

    /// Pixel budget (`maxPixelCount`); 0 means no limit.
    pub fn pixel_budget(self) -> u64 {
        match self {
            Resolution::P480 => 854 * 480,
            Resolution::P720 => 1280 * 720,
            Resolution::P1080 => 1920 * 1080,
            Resolution::P1440 => 2560 * 1440,
            Resolution::Source => 0,
        }
    }
}

/// Frame-rate options offered in the picker.
pub const FRAME_RATES: [u32; 3] = [15, 30, 60];
/// Documents preset: Source resolution at 5 fps, for sharp text.
pub const TEXT_PRESET: (Resolution, u32) = (Resolution::Source, 5);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct StreamSettings {
    pub resolution: Resolution,
    pub fps: u32,
    /// Highest tile quality (`quality`, default 100).
    pub quality: u8,
    pub share_audio: bool,
    pub adaptive: bool,
    pub motion_video: bool,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            resolution: Resolution::P1080,
            fps: 60,
            quality: DEFAULT_QUALITY,
            share_audio: false,
            adaptive: true,
            motion_video: true,
        }
    }
}

impl StreamSettings {
    pub fn dimensions(&self, width: u32, height: u32) -> (u32, u32) {
        let budget = match self.resolution.pixel_budget() {
            0 => crate::protocol::MAX_VIDEO_PIXELS,
            n => n.min(crate::protocol::MAX_VIDEO_PIXELS),
        };
        let (w, h) = fit_resolution(width, height, budget);
        let scale = (crate::protocol::MAX_VIDEO_SIDE as f64 / f64::from(w.max(h))).min(1.0);
        (
            ((f64::from(w) * scale) as u32).max(2) & !1,
            ((f64::from(h) * scale) as u32).max(2) & !1,
        )
    }
    /// 480p/720p use 3 Mb/s. Larger pictures scale with pixels and frame rate,
    /// with a 6 Mb/s floor and 40 Mb/s ceiling.
    pub fn bitrate_kbps(&self, w: u32, h: u32) -> u32 {
        (super::bitrate::ceiling(StreamKind::Screen, w, h, self.fps) / 1000).min(MAX_KBPS_LIMIT)
    }
    /// Target bitrate (`targetBitrate`) for this preset, in bits per second (estimated).
    pub fn target_bitrate(&self, w: u32, h: u32) -> u64 {
        u64::from(self.bitrate_kbps(w, h)) * 1000
    }
}

/// The local preview is refreshed at most this often.
const PREVIEW_GAP: Duration = Duration::from_micros(16_667);
/// Pixel budget for the local preview. The transmitted frame keeps its requested
/// resolution; this only avoids converting full 1080p frames for a smaller UI tile.
const PREVIEW_PIXEL_BUDGET: u64 = 720 * 405;

/// A short final spin avoids oversleeping a 60 Hz capture deadline on Windows.
/// The capture worker sleeps for most of the interval, then waits against the
/// monotonic clock for the final 2 ms.
const CAPTURE_SPIN_WINDOW: Duration = Duration::from_millis(2);

fn wait_until_capture_deadline(deadline: Instant) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining > CAPTURE_SPIN_WINDOW {
        std::thread::sleep(remaining - CAPTURE_SPIN_WINDOW);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    fit_resolution(width, height, PREVIEW_PIXEL_BUDGET)
}

type Capture = (u32, u32, Vec<u8>, PixelFormat);

/// Hand-over between the capture and the encode thread: only the newest picture
/// is kept, so a slow encoder drops old pictures instead of building latency.
#[derive(Default)]
struct LatestFrame {
    slot: Mutex<Option<Capture>>,
    ready: parking_lot::Condvar,
    error: Mutex<Option<String>>,
}

impl LatestFrame {
    fn put(&self, picture: Capture) {
        *self.slot.lock() = Some(picture);
        self.ready.notify_one();
    }
    fn fail(&self, error: String) {
        *self.error.lock() = Some(error);
        self.ready.notify_all();
    }
    fn error(&self) -> Option<String> {
        self.error.lock().take()
    }
    /// The newest picture, waiting up to `wait` for one.
    fn take(&self, wait: Duration) -> Option<Capture> {
        let mut slot = self.slot.lock();
        if slot.is_none() && self.error.lock().is_none() {
            self.ready.wait_for(&mut slot, wait);
        }
        slot.take()
    }
}

/// Frame rate at an adaptation level. Keep the requested rate while resolution
/// adapts; halve it only after the lowest resolution step still cannot keep up.
fn level_fps(fps: u32, level: u32) -> u32 {
    let factor = [1.0, 1.0, 1.0, 1.0, 0.5][level.min(MAX_LEVEL) as usize];
    let floor = 15.min(fps);
    ((fps as f64 * factor) as u32).clamp(floor.max(1), 60)
}

/// Picture size for a source size at an adaptation level. Levels 1–3 reduce
/// resolution progressively, never below MIN_ADAPTED_SIDE on the short side.
fn target_dimensions(settings: &StreamSettings, sw: u32, sh: u32, level: u32) -> (u32, u32) {
    let (requested_w, requested_h) = settings.dimensions(sw, sh);
    let reduction = [1.0, 0.85, 0.70, 0.55, 0.55][level.min(MAX_LEVEL) as usize];
    let short = requested_w.min(requested_h);
    let floor_side = MIN_ADAPTED_SIDE.min(short);
    let reduction = if f64::from(short) * reduction < f64::from(floor_side) {
        f64::from(floor_side) / f64::from(short)
    } else {
        reduction
    };
    (
        ((f64::from(requested_w) * reduction) as u32).max(2) & !1,
        ((f64::from(requested_h) * reduction) as u32).max(2) & !1,
    )
}

fn thumb(rgba: &[u8], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let s = (THUMB_W as f32 / w as f32)
        .min(THUMB_H as f32 / h as f32)
        .min(1.0);
    let tw = ((w as f32 * s) as u32).max(1);
    let th = ((h as f32 * s) as u32).max(1);
    (tw, th, scale_rgba(rgba, w, h, tw, th))
}

/// Every screen and visible window, with thumbnails when `thumbs` is set.
pub fn list_sources(thumbs: bool) -> Vec<ShareSource> {
    let mut out = Vec::new();
    if let Ok(monitors) = xcap::Monitor::all() {
        for (i, m) in monitors.iter().enumerate() {
            let Ok(id) = m.id() else { continue };
            let width = m.width().unwrap_or(0);
            let height = m.height().unwrap_or(0);
            let primary = m.is_primary().unwrap_or(false);
            let name = if primary {
                format!("Screen {} (main)", i + 1)
            } else {
                format!("Screen {}", i + 1)
            };
            let thumb = if thumbs {
                m.capture_image().ok().map(|img| {
                    let (w, h) = (img.width(), img.height());
                    thumb(img.as_raw(), w, h)
                })
            } else {
                None
            };
            out.push(ShareSource {
                process_id: None,
                id: SourceId::Monitor(id),
                name,
                app: String::new(),
                width,
                height,
                thumb,
            });
        }
    }
    if let Ok(windows) = xcap::Window::all() {
        let own_pid = std::process::id();
        for w in windows {
            if w.is_minimized().unwrap_or(true) || w.pid().ok() == Some(own_pid) {
                continue;
            }
            let title = w.title().unwrap_or_default();
            let width = w.width().unwrap_or(0);
            let height = w.height().unwrap_or(0);
            if title.trim().is_empty() || width < 120 || height < 80 {
                continue;
            }
            let Ok(id) = w.id() else { continue };
            let app = w.app_name().unwrap_or_default();
            let thumb = if thumbs {
                w.capture_image().ok().map(|img| {
                    let (iw, ih) = (img.width(), img.height());
                    thumb(img.as_raw(), iw, ih)
                })
            } else {
                None
            };
            out.push(ShareSource {
                process_id: w.pid().ok(),
                id: SourceId::Window(id),
                name: title,
                app,
                width,
                height,
                thumb,
            });
        }
    }
    out
}

fn capture(id: SourceId) -> Result<(u32, u32, Vec<u8>), String> {
    match id {
        SourceId::Monitor(mid) => {
            let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
            let m = monitors
                .into_iter()
                .find(|m| m.id().ok() == Some(mid))
                .ok_or("screen is gone")?;
            let img = m.capture_image().map_err(|e| e.to_string())?;
            Ok((img.width(), img.height(), img.into_raw()))
        }
        SourceId::Window(wid) => {
            let windows = xcap::Window::all().map_err(|e| e.to_string())?;
            let w = windows
                .into_iter()
                .find(|w| w.id().ok() == Some(wid))
                .ok_or("window was closed")?;
            if w.is_minimized().unwrap_or(false) {
                return Err("minimized".into());
            }
            let img = w.capture_image().map_err(|e| e.to_string())?;
            Ok((img.width(), img.height(), img.into_raw()))
        }
    }
}

#[cfg(windows)]
type GameCapture = super::wgc::WgcCapture;

/// Stand-in where Windows Graphics Capture does not exist.
#[cfg(not(windows))]
struct GameCapture;
#[cfg(not(windows))]
impl GameCapture {
    fn new(_: SourceId) -> Result<Self, String> {
        Err("unsupported".into())
    }
    fn grab(&mut self, _: Duration) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        Ok(None)
    }
    fn take_map_busy(&mut self) -> u32 {
        0
    }
}

/// Fast capture of a whole screen through desktop duplication, when available.
fn start_recorder(
    id: SourceId,
) -> Option<(xcap::VideoRecorder, std::sync::mpsc::Receiver<xcap::Frame>)> {
    let SourceId::Monitor(mid) = id else {
        return None;
    };
    let monitor = xcap::Monitor::all()
        .ok()?
        .into_iter()
        .find(|m| m.id().ok() == Some(mid))?;
    let (rec, rx) = monitor.video_recorder().ok()?;
    rec.start().ok()?;
    Some((rec, rx))
}

/// How often the pointer is sampled. Far above the video rate, so it moves smoothly
/// on the viewer's side whatever the picture is doing.
const CURSOR_POLL: Duration = Duration::from_millis(10);
/// The pointer image is sent again this often, so a viewer that joined late (or lost
/// the packet that carried it) still gets it.
const CURSOR_SHAPE_REPEAT: Duration = Duration::from_secs(1);

/// Samples the pointer and sends it apart from the video. The sharer's own preview is
/// fed too, so it looks the same as what viewers see.
fn spawn_cursor_thread(
    id: SourceId,
    stop: Arc<AtomicBool>,
    sender: ClientSender,
    keyframes: Arc<KeyframeFlags>,
    preview: PictureStore,
    me: UserId,
) {
    std::thread::Builder::new()
        .name("screen-share-cursor".into())
        .spawn(move || {
            let monitor_area = match id {
                SourceId::Monitor(mid) => xcap::Monitor::all()
                    .ok()
                    .and_then(|all| all.into_iter().find(|m| m.id().ok() == Some(mid)))
                    .map(|m| Rect {
                        x: m.x().unwrap_or(0),
                        y: m.y().unwrap_or(0),
                        w: m.width().unwrap_or(0),
                        h: m.height().unwrap_or(0),
                    }),
                SourceId::Window(_) => None,
            };
            let mut shape: Option<crate::protocol::CursorShape> = None;
            let mut shape_failed: Option<u32> = None;
            let mut shape_sent_id: Option<u32> = None;
            let mut shape_sent = Instant::now();
            let mut viewers_seen = 0u32;
            let mut last: Option<(u16, u16, bool, u32)> = None;
            while !stop.load(Ordering::SeqCst) && !sender.is_closed() {
                std::thread::sleep(CURSOR_POLL);
                let area = match id {
                    SourceId::Monitor(_) => monitor_area,
                    SourceId::Window(wid) => cursor::client_rect(wid),
                };
                let sample = cursor::sample();
                let placed = sample
                    .zip(area)
                    .and_then(|(s, a)| cursor::to_fraction(s, a).map(|f| (s, a, f)));
                let (x, y, visible, shape_id, area_w) = match placed {
                    Some((s, a, (x, y))) => (x, y, true, s.id, a.w),
                    None => match (last, area) {
                        // Keep the last spot while hidden, so it does not jump on reappearing.
                        (Some((x, y, _, shape_id)), Some(a)) => (x, y, false, shape_id, a.w),
                        _ => continue,
                    },
                };
                if visible
                    && shape.as_ref().map(|s| s.id) != Some(shape_id)
                    && shape_failed != Some(shape_id)
                {
                    shape = cursor::shape();
                    if shape.is_none() {
                        shape_failed = Some(shape_id);
                    }
                }
                // A pointer image that cannot be drawn (a blank one, which is how many
                // games hide the mouse) is not shown at all; without an image the
                // viewer would draw a generic dot where the game has no pointer.
                let visible = visible && shape_failed != Some(shape_id);
                let viewers = keyframes.screen_viewers.load(Ordering::Relaxed);
                let attach = visible
                    && shape.is_some()
                    && (shape_sent_id != shape.as_ref().map(|s| s.id)
                        || viewers > viewers_seen
                        || shape_sent.elapsed() >= CURSOR_SHAPE_REPEAT);
                viewers_seen = viewers;
                let key = (x, y, visible, shape_id);
                if last == Some(key) && !attach {
                    continue;
                }
                last = Some(key);
                let state = crate::protocol::CursorState {
                    x,
                    y,
                    visible,
                    area_w,
                    shape_id,
                    shape: if attach { shape.clone() } else { None },
                };
                if attach {
                    shape_sent_id = shape.as_ref().map(|s| s.id);
                    shape_sent = Instant::now();
                }
                preview.put_cursor(me, state.clone());
                if viewers > 0 {
                    sender.send_cursor(state);
                }
            }
        })
        .ok();
}

#[derive(Clone, Default)]
pub struct ShareStats {
    pub target_bps: u32,
    pub minimum_bps: u32,
    pub host_budget_exceeded: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub kbps: f32,
    pub quality: u8,
    pub ended: Option<String>,
    pub audio_active: bool,
    pub audio_error: Option<String>,
    pub capture_fps: f32,
    pub capture_events: u32,
    pub capture_busy: u32,
    pub encoded_fps: f32,
    pub dropped_frames: u64,
    pub codec: String,
    pub encode_ms: f32,
}

pub struct ScreenShare {
    stop: Arc<AtomicBool>,
    pub stats: Arc<Mutex<ShareStats>>,
    pub source_name: String,
    pub settings: StreamSettings,
    _audio: Option<super::loopback::Loopback>,
}

impl Drop for ScreenShare {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl ScreenShare {
    pub fn start(
        source: &ShareSource,
        settings: StreamSettings,
        sender: ClientSender,
        keyframes: Arc<KeyframeFlags>,
        preview: PictureStore,
        me: UserId,
        ctx: egui::Context,
    ) -> ScreenShare {
        let stop = Arc::new(AtomicBool::new(false));
        let (initial_w, initial_h) = settings.dimensions(source.width, source.height);
        let initial_max_bps = settings.target_bitrate(initial_w, initial_h) as u32;
        let initial_min_bps =
            super::bitrate::minimum_for_ceiling(StreamKind::Screen, initial_max_bps);
        let stats = Arc::new(Mutex::new(ShareStats {
            target_bps: initial_min_bps,
            minimum_bps: initial_min_bps,
            ..ShareStats::default()
        }));
        let audio = if settings.share_audio {
            match super::voice::MusicEncoder::new() {
                Ok(mut encoder) => {
                    let audio_sender = sender.clone();
                    let audio_viewers = keyframes.clone();
                    let mut sequence = 0u32;
                    let started = Instant::now();
                    Some(super::loopback::Loopback::start(
                        source.process_id,
                        move |samples| {
                            if audio_viewers.screen_viewers.load(Ordering::Relaxed) == 0 {
                                return;
                            }
                            if let Ok(packet) = encoder.encode(samples) {
                                audio_sender.send(&crate::protocol::ClientMsg::StreamAudio {
                                    seq: sequence,
                                    sent_micros: started.elapsed().as_micros() as u64,
                                    packet,
                                });
                                sequence = sequence.wrapping_add(1);
                            }
                        },
                    ))
                }
                Err(error) => {
                    stats.lock().audio_error = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let audio_health = audio
            .as_ref()
            .map(|audio| (audio.active.clone(), audio.error.clone()));
        let id = source.id;
        spawn_cursor_thread(
            id,
            stop.clone(),
            sender.clone(),
            keyframes.clone(),
            preview.clone(),
            me,
        );
        let tick_us = Arc::new(AtomicU64::new(
            (1_000_000.0 / f64::from(settings.fps.clamp(1, 60))) as u64,
        ));
        let latest = Arc::new(LatestFrame::default());
        let captured = Arc::new(AtomicU32::new(0));
        let capture_events = Arc::new(AtomicU32::new(0));
        let capture_busy = Arc::new(AtomicU32::new(0));
        // Capture thread: only grabs pictures, paced to the current tick, and leaves
        // the newest one for the encoder. A slow encoder never stalls the capture
        // (and the other way around); stale pictures are simply replaced.
        {
            let stop = stop.clone();
            let latest = latest.clone();
            let tick_us = tick_us.clone();
            let captured = captured.clone();
            let capture_events = capture_events.clone();
            let capture_busy = capture_busy.clone();
            let sender = sender.clone();
            std::thread::Builder::new()
                .name("screen-capture".into())
                .spawn(move || {
                    // Graphics Capture first: it is the only path that sees games.
                    let mut wgc = GameCapture::new(id).ok();
                    let mut recorder = if wgc.is_some() {
                        None
                    } else {
                        start_recorder(id)
                    };
                    let mut failures = 0u32;
                    // A monitor can raise WGC frame events faster than the requested
                    // stream rate (for example, a 180 Hz display for a 60 FPS stream).
                    // Sample the newest frame on an absolute cadence so we do not
                    // read back and copy every intermediate monitor refresh.
                    let mut next_wgc_sample = Instant::now();
                    while !stop.load(Ordering::SeqCst) && !sender.is_closed() {
                        let requested_tick =
                            Duration::from_micros(tick_us.load(Ordering::Relaxed).max(1000));
                        // Keep the local preview at the requested cadence even without a
                        // remote watcher. The no-viewer path still avoids stream-sized
                        // scaling and video encoding below.
                        let tick = requested_tick;
                        if wgc.is_some() {
                            wait_until_capture_deadline(next_wgc_sample);
                        }
                        let started = Instant::now();
                        let grabbed: Result<Option<Capture>, String> = match wgc
                            .as_mut()
                            .map(|c| c.grab(tick))
                        {
                            Some(Err(e)) if e != "window was closed" => {
                                // Graphics Capture broke: fall back to the older paths.
                                wgc = None;
                                recorder = start_recorder(id);
                                continue;
                            }
                            Some(result) => result.map(|picture| {
                                picture.map(|(w, h, pixels)| (w, h, pixels, PixelFormat::Bgra))
                            }),
                            None => match &recorder {
                                Some((_, rx)) => {
                                    // Desktop duplication: wait up to one tick for a change, keep the newest.
                                    match rx.recv_timeout(tick) {
                                        Ok(f) => {
                                            let mut newest = f;
                                            while let Ok(f) = rx.try_recv() {
                                                newest = f;
                                            }
                                            Ok(Some((
                                                newest.width,
                                                newest.height,
                                                newest.raw,
                                                PixelFormat::Rgba,
                                            )))
                                        }
                                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
                                        Err(_) => Err("recorder stopped".to_string()),
                                    }
                                }
                                None => capture(id)
                                    .map(|(w, h, pixels)| (w, h, pixels, PixelFormat::Rgba))
                                    .map(Some),
                            },
                        };
                        if let Some(capture) = wgc.as_mut() {
                            let events = capture.take_frame_events();
                            if events > 0 {
                                capture_events.fetch_add(events, Ordering::Relaxed);
                            }
                            let busy = capture.take_map_busy();
                            if busy > 0 {
                                capture_busy.fetch_add(busy, Ordering::Relaxed);
                            }
                        }
                        match grabbed {
                            Ok(Some((w, h, pixels, format))) => {
                                failures = 0;
                                captured.fetch_add(1, Ordering::Relaxed);
                                latest.put((w, h, pixels, format));
                            }
                            Ok(None) => {}
                            Err(e) => {
                                if recorder.take().is_some() {
                                    continue; // fall back to plain capture
                                }
                                failures += 1;
                                if e != "minimized" && failures > 20 {
                                    latest.fail(e);
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        }
                        let spent = started.elapsed();
                        if wgc.is_some() {
                            // Keep the cadence anchored to the prior deadline so OS
                            // timer overshoot does not add to every frame interval.
                            // If processing overruns the next slot, resume immediately.
                            next_wgc_sample = (next_wgc_sample + tick).max(Instant::now());
                        } else if spent < tick {
                            // Keep explicit pacing for snapshot/polled capture paths.
                            std::thread::sleep(tick - spent);
                        }
                    }
                    if let Some((rec, _)) = recorder {
                        let _ = rec.stop();
                    }
                })
                .ok();
        }
        {
            let stop = stop.clone();
            let stats = stats.clone();
            let capture_busy = capture_busy.clone();
            std::thread::Builder::new()
                .name("screen-share".into())
                .spawn(move || {
                    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
                    let mut encoder = TileEncoder::new();
                    let mut bitrate = super::bitrate::Controller::new(StreamKind::Screen,
                        initial_max_bps, Instant::now());
                    let mut motion = if settings.motion_video {
                        super::temporal::TemporalEncoder::new(
                            bitrate.target(),
                            settings.fps,
                            true,
                        )
                        .ok()
                    } else {
                        None
                    };
                    stats.lock().codec = if motion.is_some() {
                        "H.264 (iniciando)"
                    } else {
                        "JPEG por regiões"
                    }
                    .into();
                    let mut shown_backend = String::new();
                    let mut rate: Option<RateControl> = None;
                    let mut last_key = Instant::now() - KEYFRAME_INTERVAL;
                    let mut tick = Duration::from_secs_f64(1.0 / settings.fps.clamp(1, 60) as f64);
                    let mut needs_keyframe = true;
                    let mut queued_frames = 0u32;
                    let mut window = Instant::now();
                    let mut last: Option<Capture> = None;
                    let mut level = 0u32;
                    // Lowest level the adaptation may recover to (a weak PC stays light).
                    let mut floor_level = 0u32;
                    let mut adaptation = Instant::now();
                    let mut clean_windows = 0u32;
                    let mut floor_congested_windows = 0u32;
                    let mut previous_transport_drops = 0u64;
                    let mut overcommitted = false;
                    let mut dropped_window = 0u64;
                    let mut encoded_frames = 0u32;
                    // Smoothed time to scale, encode and send one picture. When it
                    // exceeds the frame period the PC cannot keep up (CPU/GPU busy with
                    // the game), and the stream steps down before it stutters.
                    let mut work_ms = 0.0f64;
                    // Byte budget refilled at the bandwidth ceiling (plus headroom for
                    // VBR overshoot). Frames are paced *before* encoding, because an
                    // encoded delta frame that is thrown away breaks the stream. The
                    // bucket may go well into debt (an IDR is large) before captures
                    // are skipped, so pacing only trims real overshoot and never
                    // turns the bitrate ceiling into a hidden frame-rate cap.
                    let mut budget =
                        f64::from(settings.bitrate_kbps(1920, 1080)) * 1000.0 / 8.0 * 1.5;
                    let mut tokens = budget * 0.5;
                    let mut token_clock = Instant::now();
                    let mut skipped_window = 0u32;
                    let mut last_idr = Instant::now() - MIN_KEYFRAME_GAP;
                    // The local preview does not need every frame.
                    let mut last_preview = Instant::now() - PREVIEW_GAP;
                    let mut preview_stale = false;
                    while !stop.load(Ordering::SeqCst) && !sender.is_closed() {
                        if let Some((active, error)) = &audio_health {
                            let mut health = stats.lock();
                            health.audio_active = active.load(Ordering::Relaxed);
                            health.audio_error = error.lock().clone();
                        }
                        let grabbed = latest.take(tick);
                        if let Some(error) = latest.error() {
                            stats.lock().ended = Some(error);
                            ctx.request_repaint();
                            break;
                        }
                        let work_started = Instant::now();
                        let watching = keyframes.screen_viewers.load(Ordering::Relaxed) > 0;
                        let mut requested = keyframes.take(StreamKind::Screen);
                        if requested && last_idr.elapsed() < MIN_KEYFRAME_GAP {
                            // Probably crossed an IDR that is already on its way:
                            // keep the request pending instead of bursting again.
                            keyframes.screen.store(true, Ordering::SeqCst);
                            requested = false;
                        }
                        let force = watching
                            && (requested
                                || (needs_keyframe && last_idr.elapsed() >= MIN_KEYFRAME_GAP)
                                || last_key.elapsed() >= KEYFRAME_INTERVAL);
                        if !watching {
                            needs_keyframe = true;
                        }
                        // Nothing changed on screen: only a due full picture needs sending
                        // (the pointer travels apart from the video).
                        let picture = match grabbed {
                            Some(p) => Some(p),
                            None if force => last.take(),
                            None => None,
                        };
                        if let Some((sw, sh, pixels, format)) = picture {
                            let (w, h) = target_dimensions(&settings, sw, sh, level);
                            // H.264 paths accept both channel orders. Convert BGRA only when
                            // a watched tile/JPEG fallback needs RGBA; the game capture path
                            // stays in its native format through encoding and preview scaling.
                            let fallback_rgba = if watching
                                && motion.is_none()
                                && format == PixelFormat::Bgra
                            {
                                Some(into_rgba(&pixels, format))
                            } else {
                                None
                            };
                            let source_pixels = fallback_rgba.as_deref().unwrap_or(&pixels);
                            let pixel_format = if fallback_rgba.is_some() {
                                PixelFormat::Rgba
                            } else {
                                format
                            };
                            // The local tile is displayed smaller than the stream. Keep a
                            // 540p-scale preview without changing the full-resolution frame
                            // used by the encoder when a viewer is connected.
                            let (pic_w, pic_h) = if watching {
                                (w, h)
                            } else {
                                preview_dimensions(w, h)
                            };
                            // Scale only when dimensions change; otherwise borrow the capture.
                            let scaled = if (pic_w, pic_h) == (sw, sh) {
                                None
                            } else {
                                Some(scale_rgba(source_pixels, sw, sh, pic_w, pic_h))
                            };
                            let pic: &[u8] = scaled.as_deref().unwrap_or(source_pixels);
                            budget = f64::from(bitrate.target()) / 8.0 * 1.5;
                            let rc = rate.get_or_insert_with(|| {
                                RateControl::new(settings.quality, u64::from(bitrate.target()))
                            });
                            rc.target_bps = u64::from(bitrate.target());
                            tokens =
                                (tokens + token_clock.elapsed().as_secs_f64() * budget).min(budget);
                            token_clock = Instant::now();
                            // Far in debt (big IDRs, sustained overshoot): skip this capture.
                            let paced_out = watching && !force && tokens < -budget * 0.25;
                            if paced_out {
                                skipped_window += 1;
                            }
                            if watching && !paced_out {
                                let encode_started = Instant::now();
                                if force {
                                    last_key = Instant::now();
                                    last_idr = Instant::now();
                                }
                                if let Some(video) = motion.as_mut() {
                                    video.set_bitrate(bitrate.target());
                                    let result = match pixel_format {
                                        PixelFormat::Rgba => video.encode(pic, w, h, force),
                                        PixelFormat::Bgra => video.encode_bgra(pic, w, h, force),
                                    };
                                    let backend = video.backend();
                                    if backend != shown_backend {
                                        let weak_cpu = backend == "CPU"
                                            && cores <= 4
                                            && (settings.fps > 30
                                                || settings.resolution.pixel_budget() == 0
                                                || settings.resolution.pixel_budget() > 1280 * 720);
                                        stats.lock().codec = if weak_cpu {
                                            format!("H.264 ({backend}, modo leve: PC modesto)")
                                        } else {
                                            format!("H.264 ({backend})")
                                        };
                                        if weak_cpu && floor_level < 2 {
                                            // No GPU encoder and few cores: start light and
                                            // never climb back to what it cannot sustain.
                                            floor_level = 2;
                                            level = level.max(2);
                                            needs_keyframe = true;
                                        }
                                        shown_backend = backend;
                                    }
                                    match result {
                                        Ok(Some(frame)) => {
                                            let bytes = frame.data.len();
                                            rc.record(bytes);
                                            // Always sent: the bucket may go into debt, which
                                            // delays the next captures instead.
                                            tokens -= bytes as f64;
                                            if frame.keyframe {
                                                last_idr = Instant::now();
                                            }
                                            needs_keyframe = sender
                                                .send_encoded_frame(StreamKind::Screen, frame);
                                            encoded_frames += 1;
                                            if !needs_keyframe {
                                                queued_frames += 1;
                                            }
                                            if needs_keyframe {
                                                dropped_window += 1;
                                                stats.lock().dropped_frames += 1;
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            // Keep the stream usable with the bounded tile codec.
                                            stats.lock().codec = format!(
                                                "JPEG por regiões (H.264 indisponível: {error})"
                                            );
                                            motion = None;
                                            needs_keyframe = true;
                                        }
                                    }
                                } else if let Some(frame) =
                                    encoder.encode(pic, w, h, rc.quality, force)
                                {
                                    rc.record(frame.byte_size());
                                    tokens -= frame.byte_size() as f64;
                                    if frame.keyframe {
                                        last_idr = Instant::now();
                                    }
                                    needs_keyframe = sender.send_frame(StreamKind::Screen, frame);
                                    encoded_frames += 1;
                                    if !needs_keyframe {
                                        queued_frames += 1;
                                    }
                                    if needs_keyframe {
                                        dropped_window += 1;
                                        stats.lock().dropped_frames += 1;
                                    }
                                } else {
                                    rc.record(0);
                                }
                                stats.lock().encode_ms =
                                    encode_started.elapsed().as_secs_f32() * 1000.0;
                            } else if !watching {
                                rc.record(0);
                            }
                            if last_preview.elapsed() >= PREVIEW_GAP {
                                let (preview_w, preview_h) = preview_dimensions(w, h);
                                let preview_scaled = if (pic_w, pic_h) == (preview_w, preview_h) {
                                    None
                                } else {
                                    Some(scale_rgba(pic, pic_w, pic_h, preview_w, preview_h))
                                };
                                let preview_pixels = preview_scaled.as_deref().unwrap_or(pic);
                                preview.put(
                                    (me, StreamKind::Screen),
                                    preview_w,
                                    preview_h,
                                    into_rgba(preview_pixels, pixel_format),
                                    0,
                                );
                                ctx.request_repaint();
                                last_preview = Instant::now();
                                preview_stale = false;
                            } else {
                                preview_stale = true;
                            }
                            let mut s = stats.lock();
                            s.width = w;
                            s.height = h;
                            s.quality = rc.quality;
                            s.kbps = rc.last_bps as f32 / 1000.0;
                            drop(s);
                            last = Some((sw, sh, pixels, format));
                            if watching {
                                let ms = work_started.elapsed().as_secs_f64() * 1000.0;
                                work_ms = if work_ms == 0.0 {
                                    ms
                                } else {
                                    work_ms * 0.9 + ms * 0.1
                                };
                            }
                        } else {
                            if let Some(rc) = rate.as_mut() {
                                rc.record(0);
                            }
                            // A preview frame skipped for pacing is shown once things go quiet.
                            if preview_stale {
                                if let Some((sw, sh, src, format)) = &last {
                                    let (w, h) = target_dimensions(&settings, *sw, *sh, level);
                                    let (preview_w, preview_h) = preview_dimensions(w, h);
                                    let pic = scale_rgba(src, *sw, *sh, preview_w, preview_h);
                                    preview.put(
                                        (me, StreamKind::Screen),
                                        preview_w,
                                        preview_h,
                                        into_rgba(&pic, *format),
                                        0,
                                    );
                                    ctx.request_repaint();
                                    last_preview = Instant::now();
                                }
                                preview_stale = false;
                            }
                        }
                        if window.elapsed() >= Duration::from_secs(1) {
                            let secs = window.elapsed().as_secs_f32();
                            let mut current = stats.lock();
                            // Count frames accepted by the bulk queue, not captures that may
                            // have been paced out or encoded to no output.
                            current.fps = queued_frames as f32 / secs;
                            current.capture_fps = captured.swap(0, Ordering::Relaxed) as f32 / secs;
                            current.capture_events = capture_events.swap(0, Ordering::Relaxed);
                            current.encoded_fps = encoded_frames as f32 / secs;
                            drop(current);
                            queued_frames = 0;
                            encoded_frames = 0;
                            window = Instant::now();
                        }
                        if adaptation.elapsed() >= Duration::from_secs(1) {
                            let busy_maps = capture_busy.swap(0, Ordering::Relaxed);
                            stats.lock().capture_busy = busy_maps;
                            let (capture_fps, encoded_fps) = {
                                let current = stats.lock();
                                (current.capture_fps, current.encoded_fps)
                            };
                            let target_fps = level_fps(settings.fps, level) as f32;
                            // Long encoder calls can drop visible game frames even when
                            // average work time stays below one frame period. When capture
                            // keeps pace, actual encoded cadence is a second signal for
                            // adaptive resolution.
                            let encoder_underproducing = watching
                                && motion.is_some()
                                && capture_fps >= target_fps * 0.90
                                && encoded_fps < target_fps * 0.95;
                            // Real congestion: the send queue overflowed, the queue is
                            // deeply backed up, or pacing swallowed most captures.
                            let slow = skipped_window > settings.fps.max(1) / 2;
                            // The PC itself cannot finish a picture within a frame period.
                            let capture_stalled = watching && busy_maps > settings.fps.max(1) / 10;
                            let overloaded = work_ms > tick.as_secs_f64() * 1000.0 * 0.9
                                || capture_stalled
                                || encoder_underproducing;
                            let transport = sender.media_stats();
                            let remote = sender.video_control(StreamKind::Screen);
                            let network_congested = remote.congested || dropped_window > 0
                                || slow
                                || transport.dropped > previous_transport_drops
                                || transport.queued_bytes > (bitrate.target() as usize / 8 / 5);
                            previous_transport_drops = transport.dropped;
                            let (w, h) = last.as_ref().map_or((1920, 1080), |(w, h, _, _)| settings.dimensions(*w, *h));
                            let max = settings.target_bitrate(w, h) as u32;
                            let cap = if remote.max_bps == 0 { max } else { max.min(remote.max_bps) };
                            let before = bitrate.target();
                            bitrate.update(cap, network_congested,
                                remote.healthy && !network_congested, Instant::now());
                            if before != bitrate.target() {
                                eprintln!("Tela: bitrate-alvo {} -> {} bit/s", before, bitrate.target());
                            }
                            if before != bitrate.target() || remote.overcommitted != overcommitted {
                                ctx.request_repaint();
                            }
                            if remote.overcommitted && !overcommitted {
                                eprintln!("Vídeo: orçamento do host insuficiente para manter todos os pisos de bitrate");
                            }
                            overcommitted = remote.overcommitted;
                            {
                                let mut current = stats.lock();
                                current.target_bps = bitrate.target();
                                current.minimum_bps = bitrate.floor();
                                current.host_budget_exceeded = overcommitted;
                            }
                            if network_congested && bitrate.target() == bitrate.floor() {
                                floor_congested_windows += 1;
                            } else { floor_congested_windows = 0; }
                            let congested = overloaded || floor_congested_windows >= 3;
                            let previous = level;
                            if settings.adaptive && congested {
                                level = (level + 1).min(MAX_LEVEL);
                                clean_windows = 0;
                            } else if settings.adaptive {
                                clean_windows += 1;
                                // Recover quickly: 3 clean seconds per step up, but only
                                // when the work measured so far would fit the next step.
                                if clean_windows >= 3 && level > floor_level {
                                    let next = level - 1;
                                    let next_tick = Duration::from_secs_f64(
                                        1.0 / f64::from(level_fps(settings.fps, next)),
                                    );
                                    let pixels_up = last.as_ref().map_or(1.0, |(sw, sh, _, _)| {
                                        let (w, h) = target_dimensions(&settings, *sw, *sh, level);
                                        let (next_w, next_h) =
                                            target_dimensions(&settings, *sw, *sh, next);
                                        (f64::from(next_w) * f64::from(next_h)
                                            / (f64::from(w) * f64::from(h)))
                                            .max(1.0)
                                    });
                                    let fits = work_ms * pixels_up
                                        < next_tick.as_secs_f64() * 1000.0 * 0.75;
                                    if fits {
                                        level = next;
                                    }
                                    clean_windows = 0;
                                }
                            }
                            level = level.max(floor_level);
                            // Only a resolution change needs a new IDR.
                            // Every transition through levels 1–3 changes the
                            // coded dimensions and therefore needs a fresh IDR.
                            if previous.min(3) != level.min(3) {
                                needs_keyframe = true;
                            }
                            tick = Duration::from_secs_f64(
                                1.0 / f64::from(level_fps(settings.fps, level)),
                            );
                            tick_us.store(tick.as_micros() as u64, Ordering::Relaxed);
                            dropped_window = 0;
                            skipped_window = 0;
                            adaptation = Instant::now();
                        }
                    }
                })
                .ok();
        }
        ScreenShare {
            stop,
            stats,
            source_name: source.name.clone(),
            settings,
            _audio: audio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_frame_keeps_only_the_newest_picture() {
        let latest = LatestFrame::default();
        latest.put((2, 2, vec![1; 16], PixelFormat::Rgba));
        latest.put((2, 2, vec![2; 16], PixelFormat::Rgba));
        let got = latest.take(Duration::from_millis(5)).unwrap();
        assert_eq!(got.2[0], 2);
        assert!(latest.take(Duration::from_millis(5)).is_none());
    }

    #[test]
    fn adaptation_levels_never_drop_below_the_fps_floor() {
        assert_eq!(level_fps(60, 0), 60);
        assert_eq!(level_fps(60, 3), 60);
        assert_eq!(level_fps(60, 4), 30);
        assert_eq!(level_fps(15, 4), 15);
        let settings = StreamSettings::default();
        let full = target_dimensions(&settings, 1920, 1080, 0);
        let light = target_dimensions(&settings, 1920, 1080, 2);
        assert_eq!(full, (1920, 1080));
        let first_adaptation = target_dimensions(&settings, 1920, 1080, 1);
        assert!(first_adaptation.0 < full.0 && first_adaptation.0.is_multiple_of(2));
        assert!(
            light.0 < first_adaptation.0 && light.0.is_multiple_of(2) && light.1.is_multiple_of(2)
        );
    }

    #[test]
    fn bitrate_scales_with_resolution_and_fps() {
        let s = StreamSettings {
            fps: 60,
            ..StreamSettings::default()
        };
        let (k720, k1080, k1440) = (
            s.bitrate_kbps(1280, 720),
            s.bitrate_kbps(1920, 1080),
            s.bitrate_kbps(2560, 1440),
        );
        assert_eq!(k720, 3000);
        assert!((11_000..=13_000).contains(&k1080), "{k1080}");
        assert!((19_000..=22_000).contains(&k1440), "{k1440}");
        let slow = StreamSettings { fps: 30, ..s };
        assert!(slow.bitrate_kbps(1920, 1080) < k1080);
        assert_eq!(s.bitrate_kbps(7680, 4320), MAX_KBPS_LIMIT);
    }

    #[test]
    fn low_resolution_presets_use_three_megabits_at_every_frame_rate() {
        for resolution in [Resolution::P480, Resolution::P720] {
            for fps in FRAME_RATES {
                let settings = StreamSettings {
                    resolution,
                    fps,
                    ..StreamSettings::default()
                };
                let (w, h) = settings.dimensions(3840, 2160);
                assert_eq!(settings.target_bitrate(w, h), 3_000_000);
            }
        }
    }

    #[test]
    fn source_resolution_fits_receiver_limits() {
        let settings = StreamSettings {
            resolution: Resolution::Source,
            ..StreamSettings::default()
        };
        for (width, height) in [(1920, 1080), (7680, 4320), (16_384, 1024)] {
            let (w, h) = settings.dimensions(width, height);
            assert!(crate::protocol::picture_bytes(w, h).is_some());
            assert!(w <= width && h <= height);
            assert_eq!((w % 2, h % 2), (0, 0));
        }
    }

    #[test]
    #[ignore = "requires an interactive desktop and captures the display"]
    fn screen_capture_round_trip() {
        let src = list_sources(false)
            .into_iter()
            .find(|s| matches!(s.id, SourceId::Monitor(_)))
            .expect("an accessible display is required");
        let started = Instant::now();
        let (sw, sh, rgba) = capture(src.id).expect("capture");
        let cap_ms = started.elapsed().as_millis();
        let (w, h) = fit_resolution(sw, sh, Resolution::P1080.pixel_budget());
        let pic = scale_rgba(&rgba, sw, sh, w, h);
        let t = Instant::now();
        let mut enc = TileEncoder::new();
        let frame = enc.encode(&pic, w, h, 85, true).unwrap();
        let enc_ms = t.elapsed().as_millis();
        let store = PictureStore::default();
        store.apply((1, StreamKind::Screen), &frame);
        let (pw, ph) = store
            .with(|m| m.get(&(1, StreamKind::Screen)).map(|p| (p.width, p.height)))
            .unwrap();
        assert_eq!((pw, ph), (w, h));
        eprintln!(
            "screen {sw}x{sh} -> {w}x{h}: capture {cap_ms} ms, encode {enc_ms} ms, {} KB",
            frame.byte_size() / 1024
        );
        if let Some((rec, rx)) = start_recorder(src.id) {
            let t = Instant::now();
            let first = rx.recv_timeout(Duration::from_secs(2));
            eprintln!(
                "desktop duplication first frame: {:?} after {} ms",
                first.as_ref().map(|f| (f.width, f.height)),
                t.elapsed().as_millis()
            );
            let _ = rec.stop();
            assert!(first.is_ok(), "desktop duplication must produce a frame");
        } else {
            eprintln!("desktop duplication unavailable; plain capture is used");
        }
    }
}
