//! Voice: microphone capture with voice activity or push-to-talk, and a mixer
//! that plays everyone in the call with their own volume.

use std::collections::{HashMap, VecDeque};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use std::sync::Arc;

use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use parking_lot::Mutex;

use super::voice::{JitterBuffer, VoiceEncoder, VoiceStats};

use crate::model::UserId;

use crate::net::client::ClientSender;

use crate::protocol::ClientMsg;
mod capture;
mod dsp;
mod engine;
mod mixer;
mod output_ring;
mod playback;
pub mod voice_note;
use capture::*;
use dsp::*;
pub use engine::*;
use playback::*;

/// Rate voice travels at over the network.
pub const RATE: u32 = 24_000;

/// 20 ms of voice at RATE.
pub const FRAME: usize = (RATE / 50) as usize;

/// Voice activity defaults: threshold -40 dB, 5 frames leading, 25 frames trailing,
/// 20 ms push-to-talk release delay (one frame is 20 ms).
pub const VAD_THRESHOLD_DB: f32 = -40.0;

const VAD_LEADING_FRAMES: usize = 5;

const VAD_TRAILING_FRAMES: u32 = 25;

const PTT_RELEASE_DELAY_MS: u32 = 20;

/// Most audio buffered per person before old audio is dropped (300 ms).
const MAX_BUFFER: usize = (RATE as usize * 3) / 10;

/// Audio buffered before a person starts playing (40 ms), to ride out jitter.
const START_BUFFER: usize = FRAME * 2;

/// Our own voice in the microphone test.
pub const SELF_TEST_ID: UserId = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputMode {
    VoiceActivity,
    PushToTalk,
    /// Voice activity that adapts to the room: an adaptive noise floor combined
    /// with RNNoise's speech detector, no threshold to tune.
    SmartVoice,
}

/// Longest run of 20-ms frames kept speaking after speech stops in smart mode.
const SMART_HANGOVER_FRAMES: u32 = 18;

const SMART_LEADING_FRAMES: usize = 8;

/// Default margin (dB) over the noise floor needed to open the smart gate.
pub const SMART_DEFAULT_MARGIN_DB: f32 = 14.0;

/// Adaptive voice activity: follows the room's noise floor and, when RNNoise
/// is running, requires it to agree that the sound is speech.
struct SmartVad {
    floor_db: f32,
    active: bool,
    hangover: u32,
    onset_frames: u32,
    frames: u32,
}

fn f32_load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

fn f32_store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed)
}

/// State the audio threads and the interface share.
pub struct AudioShared {
    pub muted: AtomicBool,
    pub deafened: AtomicBool,
    push_to_talk: AtomicBool,
    smart_vad: AtomicBool,
    threshold_db: AtomicU32,
    smart_margin_db: AtomicU32,
    level_db: AtomicU32,
    pub speaking: AtomicBool,
    /// Push-to-talk key held, as seen by the window.
    pub ptt_window: AtomicBool,
    /// Windows virtual-key code for push-to-talk, checked even when the window is in the background.
    pub ptt_vk: AtomicU32,
    pub mic_test: AtomicBool,
    pub sender: Mutex<Option<ClientSender>>,
    pub error: Mutex<Option<String>>,
    seq: AtomicU32,
    pub input_gain: AtomicU32,
    device_fault: AtomicBool,
    pub processor: Arc<super::processing::AudioProcessor>,
}

/// Windowed-sinc resampler (Kaiser-like Blackman window, 8 zero crossings per
/// side at the lower of the two rates). Replaces linear interpolation, which
/// aliased the microphone when decimating and imaged the voice when upsampling.
struct Resampler<const C: usize> {
    /// Input samples consumed per output sample.
    step: f64,
    pos: f64,
    half: usize,
    hist: VecDeque<[f32; C]>,
    table: Vec<f32>,
}

const RS_PHASES: usize = 256;

const RS_ZEROS: f64 = 8.0;

struct PersonBuffer {
    queue: VecDeque<f32>,
    playing: bool,
    encoded: Option<JitterBuffer>,
}

struct StreamBuffer {
    queue: VecDeque<[f32; 2]>,
    jitter: JitterBuffer,
}

/// Mixes the voices of everyone in the call.
#[derive(Default)]
pub struct Mixer {
    people: Mutex<HashMap<UserId, PersonBuffer>>,
    streams: Mutex<HashMap<UserId, StreamBuffer>>,
    /// Per-person volume (1.0 = 100 %) and local mute.
    settings: Mutex<HashMap<UserId, (f32, bool)>>,
    /// Per-sharer volume and local mute for screen-share audio.
    stream_settings: Mutex<HashMap<UserId, (f32, bool)>>,
    pub deafened: AtomicBool,
    output_volume: AtomicU32,
    device_fault: AtomicBool,
    pub underrun_frames: std::sync::atomic::AtomicU64,
    processor: Mutex<Option<Arc<super::processing::AudioProcessor>>>,
}

pub struct AudioEngine {
    stop: Arc<AtomicBool>,
}

/// Turns device-rate mono samples into 20 ms frames at RATE and decides what to send.
struct CaptureState {
    shared: Arc<AudioShared>,
    mixer: Arc<Mixer>,
    resampler: Resampler<1>,
    frame: Vec<f32>,
    quiet_frames: u32,
    was_speaking: bool,
    /// Recent frames kept so the start of a sentence isn't cut off.
    lead_in: std::collections::VecDeque<Vec<i16>>,
    ptt_release: u32,
    smart: SmartVad,
    encoder: VoiceEncoder,
    started: Instant,
}

/// Resamples the mixer's RATE output to the device rate.
struct PlaybackState {
    mixer: Arc<Mixer>,
    voice: Resampler<1>,
    buf: Vec<f32>,
    idx: usize,
    music: Resampler<2>,
    music_buf: Vec<[f32; 2]>,
    music_idx: usize,
    last_music: [f32; 2],
    render: Vec<f32>,
}

#[cfg(test)]
mod tests;
