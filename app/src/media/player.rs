//! Video playback for chat attachments and links, on top of Windows Media
//! Foundation's source reader (so the codecs are the ones the system already
//! ships: H.264, HEVC and VP9 when installed, WMV, ...).
//!
//! A decoder thread owns the reader, converts frames to RGBA for the UI and
//! feeds the audio into a small cpal output stream. Frames are paced by the
//! wall clock, anchored on the first frame shown after start/seek/resume.
//! The thread pauses itself when the UI stops drawing the player (scrolled
//! out of view), and stops when the `VideoPlayer` is dropped.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Where the video comes from.
pub enum VideoSource {
    /// A file already downloaded into memory (`name` helps container detection).
    Bytes { name: String, data: Vec<u8> },
    /// A direct http(s) link to a video file.
    Url(String),
    /// A file on disk (large attachments downloaded to the media cache); read in place, so
    /// it costs no memory beyond what the decoder buffers.
    File(std::path::PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayState {
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

pub struct VideoFrame {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub seq: u64,
}

const NO_SEEK: i64 = -1;

struct Shared {
    stop: AtomicBool,
    state: AtomicU8,
    want_paused: AtomicBool,
    /// Pending seek target in 100 ns ticks, or `NO_SEEK`.
    seek_to: AtomicI64,
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    volume: AtomicU32,
    muted: AtomicBool,
    audio_only: AtomicBool,
    frame: Mutex<Option<Arc<VideoFrame>>>,
    error: Mutex<String>,
    epoch: Instant,
    last_touch_ms: AtomicU64,
}

impl Shared {
    fn set_state(&self, s: PlayState) {
        self.state.store(s as u8, Ordering::Relaxed);
    }
    fn fail(&self, message: String) {
        *self.error.lock() = message;
        self.set_state(PlayState::Failed);
    }
}

pub struct VideoPlayer {
    shared: Arc<Shared>,
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl VideoPlayer {
    pub fn start(source: VideoSource) -> Self {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            state: AtomicU8::new(PlayState::Loading as u8),
            want_paused: AtomicBool::new(false),
            seek_to: AtomicI64::new(NO_SEEK),
            position_ms: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            volume: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            audio_only: AtomicBool::new(false),
            frame: Mutex::new(None),
            error: Mutex::new(String::new()),
            epoch: Instant::now(),
            last_touch_ms: AtomicU64::new(0),
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("video-player".into())
            .spawn(move || imp::run(worker, source));
        if spawned.is_err() {
            shared.fail("Não foi possível iniciar o reprodutor".into());
        }
        Self { shared }
    }

    /// Call on every frame the player is drawn; a player that is not drawn
    /// for a while pauses itself.
    pub fn touch(&self) {
        let ms = self.shared.epoch.elapsed().as_millis() as u64;
        self.shared.last_touch_ms.store(ms, Ordering::Relaxed);
    }

    pub fn state(&self) -> PlayState {
        match self.shared.state.load(Ordering::Relaxed) {
            0 => PlayState::Loading,
            1 => PlayState::Playing,
            2 => PlayState::Paused,
            3 => PlayState::Ended,
            _ => PlayState::Failed,
        }
    }

    pub fn error(&self) -> String {
        self.shared.error.lock().clone()
    }

    pub fn position(&self) -> Duration {
        Duration::from_millis(self.shared.position_ms.load(Ordering::Relaxed))
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.shared.duration_ms.load(Ordering::Relaxed))
    }

    /// The newest frame, if it differs from the one with sequence `seen`.
    pub fn frame_after(&self, seen: u64) -> Option<Arc<VideoFrame>> {
        self.shared
            .frame
            .lock()
            .as_ref()
            .filter(|f| f.seq != seen)
            .cloned()
    }

    pub fn toggle(&self) {
        match self.state() {
            PlayState::Ended => {
                self.shared.seek_to.store(0, Ordering::Relaxed);
                self.shared.want_paused.store(false, Ordering::Relaxed);
            }
            PlayState::Failed => {}
            _ => {
                let paused = self.shared.want_paused.load(Ordering::Relaxed);
                self.shared.want_paused.store(!paused, Ordering::Relaxed);
            }
        }
    }

    pub fn pause(&self) {
        self.shared.want_paused.store(true, Ordering::Relaxed);
    }

    pub fn seek(&self, to: Duration) {
        let ticks = (to.as_nanos() / 100) as i64;
        self.shared.seek_to.store(ticks, Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    pub fn audio_only(&self) -> bool {
        self.shared.audio_only.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    /// Playback gain, 0.0..=1.0 (applied in the audio callback; independent of mute).
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.shared.volume.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// Decodes one representative frame (a little into the video, so it is rarely
/// black), at most 480 px wide. Blocking: call from a worker thread.
pub fn thumbnail(source: VideoSource) -> Result<VideoFrame, String> {
    imp::grab(source)
}

/// Downloads a small http(s) resource (used for YouTube thumbnails). Blocking.
pub fn download(url: &str) -> Result<Vec<u8>, String> {
    imp::download(url)
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn grab(_: VideoSource) -> Result<VideoFrame, String> {
        Err("indisponível".into())
    }
    pub fn download(_: &str) -> Result<Vec<u8>, String> {
        Err("indisponível".into())
    }
    pub fn run(shared: Arc<Shared>, _source: VideoSource) {
        shared.fail("A reprodução de vídeo só está disponível no Windows".into());
    }
}

#[cfg(windows)]
mod imp;

#[cfg(all(test, windows))]
mod tests;
