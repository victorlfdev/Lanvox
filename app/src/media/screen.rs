//! Screen sharing: listing screens and windows with thumbnails, and the capture
//! loop that sends a share at the chosen quality.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::codec::{fit_resolution, scale_rgba, RateControl, TileEncoder};
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
/// Old default of the bandwidth slider; far too low for 1080p screen content.
const LEGACY_MAX_KBPS: u32 = 6000;
/// Current default of the bandwidth slider.
pub const DEFAULT_MAX_KBPS: u32 = 12_000;
/// Deepest adaptation step. Steps trade fps first, then a bounded resolution cut,
/// so quality never collapses to a blurry thumbnail.
const MAX_LEVEL: u32 = 3;
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
            Resolution::Source => "Source",
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
    pub max_bitrate_kbps: u32,
    pub motion_video: bool,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            resolution: Resolution::P1080,
            fps: 30,
            quality: DEFAULT_QUALITY,
            share_audio: false,
            adaptive: true,
            max_bitrate_kbps: DEFAULT_MAX_KBPS,
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
    /// The bandwidth ceiling in kbit/s. Settings saved with the old 6 Mbit/s
    /// default are lifted to the new default (blocky 1080p otherwise).
    pub fn max_kbps(&self) -> u32 {
        let kbps = if self.max_bitrate_kbps == LEGACY_MAX_KBPS {
            DEFAULT_MAX_KBPS
        } else {
            self.max_bitrate_kbps
        };
        kbps.clamp(500, 40_000)
    }
    /// Target bitrate (`targetBitrate`) for this preset, in bits per second (estimated).
    pub fn target_bitrate(&self, w: u32, h: u32) -> u64 {
        let pixels = (w as u64 * h as u64).max(1);
        // About 0.15 bits per pixel per frame (baseline CAVLC needs it for
        // screen content), with a floor.
        ((pixels as f64 * self.fps as f64 * 0.15) as u64)
            .clamp(1_500_000, 40_000_000)
            .min(u64::from(self.max_kbps()) * 1000)
    }
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
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub kbps: f32,
    pub quality: u8,
    pub ended: Option<String>,
    pub audio_active: bool,
    pub audio_error: Option<String>,
    pub capture_fps: f32,
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
        let stats = Arc::new(Mutex::new(ShareStats::default()));
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
        {
            let stop = stop.clone();
            let stats = stats.clone();
            std::thread::Builder::new()
                .name("screen-share".into())
                .spawn(move || {
                    let mut encoder = TileEncoder::new();
                    let mut motion = if settings.motion_video {
                        super::temporal::TemporalEncoder::new(
                            settings.max_kbps() * 1000,
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
                    let mut frames = 0u32;
                    let mut window = Instant::now();
                    let mut failures = 0u32;
                    // Graphics Capture first: it is the only path that sees games.
                    let mut wgc = GameCapture::new(id).ok();
                    let mut recorder = if wgc.is_some() {
                        None
                    } else {
                        start_recorder(id)
                    };
                    let mut last: Option<(u32, u32, Vec<u8>)> = None;
                    let mut level = 0u32;
                    let mut adaptation = Instant::now();
                    let mut clean_windows = 0u32;
                    let mut dropped_window = 0u64;
                    let mut encoded_frames = 0u32;
                    // Byte budget refilled at the bandwidth ceiling (plus headroom for
                    // VBR overshoot). Frames are paced *before* encoding, because an
                    // encoded delta frame that is thrown away breaks the stream.
                    let budget = f64::from(settings.max_kbps()) * 1000.0 / 8.0 * 1.25;
                    let mut tokens = budget * 0.5;
                    let mut token_clock = Instant::now();
                    let mut skipped_window = 0u32;
                    let mut last_idr = Instant::now() - MIN_KEYFRAME_GAP;
                    while !stop.load(Ordering::SeqCst) && !sender.is_closed() {
                        if let Some((active, error)) = &audio_health {
                            let mut health = stats.lock();
                            health.audio_active = active.load(Ordering::Relaxed);
                            health.audio_error = error.lock().clone();
                        }
                        let started = Instant::now();
                        let grabbed = match wgc.as_mut().map(|c| c.grab(tick)) {
                            Some(Err(e)) if e != "window was closed" => {
                                // Graphics Capture broke: fall back to the older paths.
                                wgc = None;
                                recorder = start_recorder(id);
                                continue;
                            }
                            Some(result) => result,
                            None => match &recorder {
                                Some((_, rx)) => {
                                    // Desktop duplication: wait up to one tick for a change, keep the newest.
                                    match rx.recv_timeout(tick) {
                                        Ok(f) => {
                                            let mut newest = f;
                                            while let Ok(f) = rx.try_recv() {
                                                newest = f;
                                            }
                                            Ok(Some((newest.width, newest.height, newest.raw)))
                                        }
                                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
                                        Err(_) => Err("recorder stopped".to_string()),
                                    }
                                }
                                None => capture(id).map(Some),
                            },
                        };
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
                        let picture = match grabbed {
                            Ok(Some(p)) => {
                                failures = 0;
                                Some(p)
                            }
                            // Nothing changed on screen: only a due full picture needs sending
                            // (the pointer travels apart from the video).
                            Ok(None) => {
                                if force {
                                    last.take()
                                } else {
                                    None
                                }
                            }
                            Err(e) => {
                                if recorder.take().is_some() {
                                    continue; // fall back to plain capture
                                }
                                failures += 1;
                                if e != "minimized" && failures > 20 {
                                    stats.lock().ended = Some(e);
                                    ctx.request_repaint();
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                                None
                            }
                        };
                        if let Some((sw, sh, rgba)) = picture {
                            let (requested_w, requested_h) = settings.dimensions(sw, sh);
                            // Levels 2 and 3 cut the resolution by 25%, never below
                            // MIN_ADAPTED_SIDE on the short side.
                            let reduction = if level >= 2 { 0.75f64 } else { 1.0 };
                            let short = requested_w.min(requested_h);
                            let floor_side = MIN_ADAPTED_SIDE.min(short);
                            let reduction = if f64::from(short) * reduction < f64::from(floor_side)
                            {
                                f64::from(floor_side) / f64::from(short)
                            } else {
                                reduction
                            };
                            let w = ((f64::from(requested_w) * reduction) as u32).max(2) & !1;
                            let h = ((f64::from(requested_h) * reduction) as u32).max(2) & !1;
                            let pic = if (w, h) == (sw, sh) {
                                rgba.clone()
                            } else {
                                scale_rgba(&rgba, sw, sh, w, h)
                            };
                            last = Some((sw, sh, rgba));
                            let rc = rate.get_or_insert_with(|| {
                                RateControl::new(settings.quality, settings.target_bitrate(w, h))
                            });
                            tokens =
                                (tokens + token_clock.elapsed().as_secs_f64() * budget).min(budget);
                            token_clock = Instant::now();
                            // In debt (a big IDR was just sent): skip this capture.
                            let paced_out = watching && !force && tokens < 0.0;
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
                                    let result = video.encode(&pic, w, h, force);
                                    let backend = video.backend();
                                    if backend != shown_backend {
                                        stats.lock().codec = format!("H.264 ({backend})");
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
                                    encoder.encode(&pic, w, h, rc.quality, force)
                                {
                                    rc.record(frame.byte_size());
                                    tokens -= frame.byte_size() as f64;
                                    if frame.keyframe {
                                        last_idr = Instant::now();
                                    }
                                    needs_keyframe = sender.send_frame(StreamKind::Screen, frame);
                                    encoded_frames += 1;
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
                            preview.put((me, StreamKind::Screen), w, h, pic, 0);
                            ctx.request_repaint();
                            frames += 1;
                            let mut s = stats.lock();
                            s.width = w;
                            s.height = h;
                            s.quality = rc.quality;
                            s.kbps = rc.last_bps as f32 / 1000.0;
                        } else if let Some(rc) = rate.as_mut() {
                            rc.record(0);
                        }
                        if window.elapsed() >= Duration::from_secs(1) {
                            let mut current = stats.lock();
                            current.fps = frames as f32 / window.elapsed().as_secs_f32();
                            current.capture_fps = current.fps;
                            current.encoded_fps =
                                encoded_frames as f32 / window.elapsed().as_secs_f32();
                            drop(current);
                            frames = 0;
                            encoded_frames = 0;
                            window = Instant::now();
                        }
                        if settings.adaptive && adaptation.elapsed() >= Duration::from_secs(1) {
                            // Real congestion: the send queue overflowed, the queue is
                            // deeply backed up, or pacing swallowed most captures.
                            let slow = skipped_window > settings.fps.max(1) / 2;
                            let congested = dropped_window > 0
                                || slow
                                || sender.media_stats().queued_bytes
                                    > settings.max_kbps() as usize * 128;
                            let previous = level;
                            if congested {
                                level = (level + 1).min(MAX_LEVEL);
                                clean_windows = 0;
                            } else {
                                clean_windows += 1;
                                // Recover quickly: 3 clean seconds per step up.
                                if clean_windows >= 3 && level > 0 {
                                    level -= 1;
                                    clean_windows = 0;
                                }
                            }
                            // Only a resolution change needs a new IDR.
                            if (previous >= 2) != (level >= 2) {
                                needs_keyframe = true;
                            }
                            let factor = [1.0, 0.75, 0.75, 0.5][level as usize];
                            let floor = 15.min(settings.fps);
                            let fps =
                                ((settings.fps as f64 * factor) as u32).clamp(floor.max(1), 60);
                            tick = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
                            dropped_window = 0;
                            skipped_window = 0;
                            adaptation = Instant::now();
                        }
                        let spent = started.elapsed();
                        if spent < tick {
                            std::thread::sleep(tick - spent);
                        }
                    }
                    if let Some((rec, _)) = recorder {
                        let _ = rec.stop();
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
