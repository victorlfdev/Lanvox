//! WebRTC AEC3 through Sonora, plus noise suppression by either Sonora's
//! WebRTC suppressor or RNNoise (neural, via nnnoiseless). Playback is the echo
//! reference, never a noise gate. Output callbacks enqueue bounded work only.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use crossbeam_channel::{bounded, Receiver, Sender};
use nnnoiseless::DenoiseState;
use parking_lot::Mutex;
use sonora::config::{EchoCanceller, EchoCanceller3Config, NoiseSuppression, TransparentModeType};
use sonora::{AudioProcessing, Config, StreamConfig};

const BLOCK: usize = 240; // 10 ms at 24 kHz.
const RNNOISE_FRAME: usize = BLOCK * 2; // RNNoise works on 10 ms at 48 kHz.
const TAPS: usize = 31;
/// Far-end audio above this RMS counts as playback that may leak into the mic.
const FAR_END_RMS: f32 = 0.002;
/// How long (10-ms blocks) AEC stays engaged after playback goes quiet, to
/// cover the echo tail and device latency.
const FAR_END_HOLD_BLOCKS: u32 = 60;
/// Envelope history (10-ms blocks) used to tell whether playback really leaks
/// into the microphone, and the longest acoustic delay searched (150 ms).
const HISTORY_BLOCKS: usize = 300;
const MAX_LAG_BLOCKS: usize = 15;
/// Envelope correlation above this means the speakers are heard by the mic.
const ECHO_ON_CORRELATION: f32 = 0.4;
const ECHO_OFF_CORRELATION: f32 = 0.2;

/// Pearson correlation of two equally long series.
fn correlation(a: &[f32], b: &[f32]) -> Option<f32> {
    let n = a.len().min(b.len());
    if n < 50 {
        return None;
    }
    let (a, b) = (&a[..n], &b[..n]);
    let mean_a = a.iter().sum::<f32>() / n as f32;
    let mean_b = b.iter().sum::<f32>() / n as f32;
    let (mut cov, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - mean_a, y - mean_b);
        cov += dx * dy;
        var_a += dx * dx;
        var_b += dy * dy;
    }
    if var_a < 1e-9 || var_b < 1e-9 {
        return None;
    }
    Some(cov / (var_a * var_b).sqrt())
}

/// AEC3 tuned to preserve the near-end voice: double talk is detected sooner
/// and suppression is less aggressive than the WebRTC defaults.
fn echo_tuning() -> EchoCanceller3Config {
    let mut tuning = EchoCanceller3Config::default();
    tuning.suppressor.normal_tuning.mask_lf.enr_transparent = 0.45;
    tuning.suppressor.normal_tuning.mask_lf.enr_suppress = 0.6;
    tuning.suppressor.normal_tuning.mask_hf.enr_transparent = 0.12;
    tuning.suppressor.normal_tuning.mask_hf.enr_suppress = 0.18;
    tuning.suppressor.normal_tuning.max_dec_factor_lf = 0.5;
    tuning.suppressor.nearend_tuning.max_dec_factor_lf = 0.5;
    tuning.suppressor.dominant_nearend_detection.enr_threshold = 0.5;
    tuning.suppressor.dominant_nearend_detection.snr_threshold = 20.0;
    tuning
        .suppressor
        .dominant_nearend_detection
        .trigger_threshold = 8;
    tuning.suppressor.dominant_nearend_detection.hold_duration = 30;
    tuning.echo_removal_control.transparent_mode = TransparentModeType::Hmm;
    tuning
}

/// Linear-phase low-pass at 48 kHz (cutoff ~11.5 kHz) shared by the 2x
/// up- and down-samplers around RNNoise.
fn lowpass_taps() -> [f32; TAPS] {
    let cutoff = 11_500.0 / 48_000.0;
    let mid = (TAPS / 2) as f32;
    let mut taps = [0.0; TAPS];
    let mut sum = 0.0;
    for (index, tap) in taps.iter_mut().enumerate() {
        let x = index as f32 - mid;
        let sinc = if x == 0.0 {
            2.0 * cutoff
        } else {
            (std::f32::consts::TAU * cutoff * x).sin() / (std::f32::consts::PI * x)
        };
        let window = 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / (TAPS - 1) as f32).cos();
        *tap = sinc * window;
        sum += *tap;
    }
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

struct Fir {
    taps: [f32; TAPS],
    history: [f32; TAPS],
}

impl Fir {
    fn new() -> Self {
        Self {
            taps: lowpass_taps(),
            history: [0.0; TAPS],
        }
    }

    fn push(&mut self, sample: f32) -> f32 {
        self.history.copy_within(1.., 0);
        self.history[TAPS - 1] = sample;
        self.history
            .iter()
            .rev()
            .zip(&self.taps)
            .map(|(a, b)| a * b)
            .sum()
    }
}

/// RNNoise stage for one 10-ms block of 24 kHz mono audio.
struct Rnnoise {
    state: Box<DenoiseState<'static>>,
    up: Fir,
    down: Fir,
}

impl Rnnoise {
    fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            up: Fir::new(),
            down: Fir::new(),
        }
    }

    /// Returns RNNoise's speech probability for the block.
    fn process(&mut self, block: &mut [f32; BLOCK]) -> f32 {
        // RNNoise expects 16-bit-range floats at 48 kHz.
        let mut input = [0.0f32; RNNOISE_FRAME];
        for (index, sample) in block.iter().enumerate() {
            input[index * 2] = self.up.push(*sample * 2.0 * 32_768.0);
            input[index * 2 + 1] = self.up.push(0.0);
        }
        let mut output = [0.0f32; RNNOISE_FRAME];
        let probability = self.state.process_frame(&mut output, &input);
        for (index, sample) in block.iter_mut().enumerate() {
            let kept = self.down.push(output[index * 2]);
            self.down.push(output[index * 2 + 1]);
            *sample = (kept / 32_768.0).clamp(-1.0, 1.0);
        }
        probability
    }
}

struct CaptureEngine {
    processor: AudioProcessing,
    reference: VecDeque<f32>,
    configured: (bool, bool),
    rnnoise: Option<Rnnoise>,
    far_end_hold: u32,
    far_rms: f32,
    far_history: VecDeque<f32>,
    mic_history: VecDeque<f32>,
    /// True when playback measurably leaks into the microphone (speakers).
    echo_present: bool,
    /// 0 = untouched microphone, 1 = fully echo-cancelled; ramped smoothly.
    wet: f32,
}

impl CaptureEngine {
    /// Records this block's envelopes and re-estimates whether the speakers
    /// are audible in the microphone. Headphones give no correlation, so the
    /// canceller then stays out of the way instead of pumping the voice.
    fn track_coupling(&mut self, mic_rms: f32) {
        for (history, value) in [
            (&mut self.far_history, self.far_rms),
            (&mut self.mic_history, mic_rms),
        ] {
            history.push_back(value);
            while history.len() > HISTORY_BLOCKS + MAX_LAG_BLOCKS {
                history.pop_front();
            }
        }
        if self.mic_history.len() < HISTORY_BLOCKS / 2 {
            return;
        }
        let far = self.far_history.make_contiguous().to_vec();
        let mic = self.mic_history.make_contiguous().to_vec();
        let mut best: Option<f32> = None;
        for lag in 0..=MAX_LAG_BLOCKS {
            // The mic hears playback `lag` blocks after it was rendered.
            let n = far.len().saturating_sub(lag);
            if let Some(c) = correlation(&far[..n], &mic[lag..lag + n]) {
                best = Some(best.map_or(c, |b: f32| b.max(c)));
            }
        }
        if let Some(best) = best {
            if best > ECHO_ON_CORRELATION {
                self.echo_present = true;
            } else if best < ECHO_OFF_CORRELATION {
                self.echo_present = false;
            }
        }
    }
}

pub struct AudioProcessor {
    capture: Mutex<CaptureEngine>,
    render_tx: Sender<Vec<f32>>,
    render_rx: Receiver<Vec<f32>>,
    echo: AtomicBool,
    noise: AtomicBool,
    use_rnnoise: AtomicBool,
    speech_probability: AtomicU32,
    delay_ms: AtomicU32,
}

fn config(echo: bool, noise: bool) -> Config {
    Config {
        echo_canceller: echo.then(EchoCanceller::default),
        noise_suppression: noise.then(NoiseSuppression::default),
        ..Config::default()
    }
}

impl AudioProcessor {
    pub fn new() -> Arc<Self> {
        let (render_tx, render_rx) = bounded(64);
        Arc::new(Self {
            capture: Mutex::new(CaptureEngine {
                processor: AudioProcessing::builder()
                    .config(config(true, true))
                    .echo_canceller3_config(echo_tuning())
                    .capture_config(StreamConfig::new(24_000, 1))
                    .render_config(StreamConfig::new(24_000, 1))
                    .build(),
                reference: VecDeque::new(),
                configured: (true, true),
                rnnoise: None,
                far_end_hold: 0,
                far_rms: 0.0,
                far_history: VecDeque::new(),
                mic_history: VecDeque::new(),
                echo_present: false,
                wet: 0.0,
            }),
            render_tx,
            render_rx,
            echo: AtomicBool::new(true),
            noise: AtomicBool::new(true),
            use_rnnoise: AtomicBool::new(true),
            speech_probability: AtomicU32::new((-1.0f32).to_bits()),
            delay_ms: AtomicU32::new(40),
        })
    }

    pub fn configure(&self, echo_cancellation: bool, noise_suppression: bool) {
        self.echo.store(echo_cancellation, Ordering::Relaxed);
        self.noise.store(noise_suppression, Ordering::Relaxed);
    }

    /// Chooses RNNoise instead of the WebRTC suppressor when noise suppression
    /// is enabled.
    pub fn set_rnnoise(&self, enabled: bool) {
        self.use_rnnoise.store(enabled, Ordering::Relaxed);
    }

    /// RNNoise speech probability (0..1) of the last processed audio, or a
    /// negative value when RNNoise is not running.
    pub fn speech_probability(&self) -> f32 {
        f32::from_bits(self.speech_probability.load(Ordering::Relaxed))
    }

    pub fn set_delay_ms(&self, delay: u32) {
        self.delay_ms.store(delay.min(500), Ordering::Relaxed);
    }

    /// The mixed mono playback signal at 24 kHz, before device resampling.
    pub fn render(&self, samples: &[f32]) {
        if samples.is_empty() || samples.len() > 4800 {
            return;
        }
        let samples: Vec<_> = samples
            .iter()
            .map(|sample| {
                if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        if let Err(crossbeam_channel::TrySendError::Full(samples)) =
            self.render_tx.try_send(samples)
        {
            let _ = self.render_rx.try_recv();
            let _ = self.render_tx.try_send(samples);
        }
    }

    /// Processes complete 10-ms blocks in place. Capture and render never share
    /// device callbacks; only this capture-side method runs the DSP pipeline.
    pub fn process_capture(&self, samples: &mut [f32]) -> Result<(), String> {
        if samples.is_empty() || !samples.len().is_multiple_of(BLOCK) || samples.len() > 4800 {
            return Err("O áudio deve conter blocos completos de 10 ms a 24 kHz.".into());
        }
        let noise = self.noise.load(Ordering::Relaxed);
        let rnnoise = noise && self.use_rnnoise.load(Ordering::Relaxed);
        // Sonora's suppressor is off whenever RNNoise runs, to avoid double
        // processing.
        let configured = (self.echo.load(Ordering::Relaxed), noise && !rnnoise);
        let mut engine = self.capture.lock();
        if engine.configured != configured {
            engine
                .processor
                .apply_config(config(configured.0, configured.1));
            engine.configured = configured;
        }
        if rnnoise && engine.rnnoise.is_none() {
            engine.rnnoise = Some(Rnnoise::new());
        } else if !rnnoise {
            engine.rnnoise = None;
        }
        engine.far_rms = 0.0;
        for chunk in self.render_rx.try_iter() {
            engine.reference.extend(chunk);
        }
        while engine.reference.len() > 4800 {
            engine.reference.pop_front();
        }
        while engine.reference.len() >= BLOCK {
            let mut reference = [0.0; BLOCK];
            for sample in &mut reference {
                *sample = engine.reference.pop_front().unwrap_or(0.0);
            }
            let mut output = [0.0; BLOCK];
            let rms = (reference.iter().map(|v| v * v).sum::<f32>() / BLOCK as f32).sqrt();
            engine.far_rms = rms;
            if rms > FAR_END_RMS {
                engine.far_end_hold = FAR_END_HOLD_BLOCKS;
            }
            engine
                .processor
                .process_render_f32(&[&reference], &mut [&mut output])
                .map_err(|error| error.to_string())?;
        }
        engine
            .processor
            .set_stream_delay_ms(self.delay_ms.load(Ordering::Relaxed) as i32)
            .map_err(|error| error.to_string())?;
        let mut probability_sum = 0.0;
        let mut blocks = 0;
        for chunk in samples.chunks_mut(BLOCK) {
            let mut input = [0.0; BLOCK];
            for (destination, sample) in input.iter_mut().zip(chunk.iter()) {
                *destination = if sample.is_finite() {
                    sample.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
            }
            let mut output = [0.0; BLOCK];
            engine
                .processor
                .process_capture_f32(&[&input], &mut [&mut output])
                .map_err(|error| error.to_string())?;
            // With no playback to leak into the microphone there is nothing to
            // cancel, so the voice is left untouched instead of being run
            // through the echo suppressor.
            let echo_enabled = configured.0;
            let mic_rms = (input.iter().map(|v| v * v).sum::<f32>() / BLOCK as f32).sqrt();
            engine.track_coupling(mic_rms);
            if echo_enabled {
                // Cancel only while playback is leaking into the mic; fade
                // between the dry and cancelled signal so nothing pumps.
                let target = if engine.echo_present && engine.far_end_hold > 0 {
                    1.0
                } else {
                    0.0
                };
                let step = 1.0 / (BLOCK as f32 * 10.0); // ~100 ms full fade
                for (out, dry) in output.iter_mut().zip(input.iter()) {
                    let wet = &mut engine.wet;
                    if *wet < target {
                        *wet = (*wet + step).min(target);
                    } else if *wet > target {
                        *wet = (*wet - step).max(target);
                    }
                    *out = *dry + (*out - *dry) * *wet;
                }
            }
            engine.far_end_hold = engine.far_end_hold.saturating_sub(1);
            if let Some(stage) = engine.rnnoise.as_mut() {
                probability_sum += stage.process(&mut output);
                blocks += 1;
            }
            chunk.copy_from_slice(&output);
        }
        let probability = if blocks > 0 {
            probability_sum / blocks as f32
        } else {
            -1.0
        };
        self.speech_probability
            .store(probability.to_bits(), Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_handles_twenty_ms_voice_with_playback_reference() {
        let processor = AudioProcessor::new();
        for frame in 0..80 {
            let mut input: Vec<_> = (0..480)
                .map(|sample| ((frame * 480 + sample) as f32 * 0.04).sin() * 0.1)
                .collect();
            processor.render(&input);
            processor.process_capture(&mut input).unwrap();
            assert_eq!(input.len(), 480);
            assert!(input
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
        }
    }

    #[test]
    fn rnnoise_attenuates_steady_noise_and_stays_finite() {
        let processor = AudioProcessor::new();
        processor.configure(false, true);
        processor.set_rnnoise(true);
        let mut random = 3u32;
        let (mut before, mut after) = (0.0f64, 0.0f64);
        for frame in 0..300 {
            let mut block = [0.0f32; 240];
            for sample in &mut block {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *sample = (random as f32 / u32::MAX as f32 - 0.5) * 0.1;
            }
            if frame >= 200 {
                before += block.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
            processor.process_capture(&mut block).unwrap();
            assert!(block.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            if frame >= 200 {
                after += block.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
        }
        assert!(after < before * 0.25, "before={before}, after={after}");
    }

    #[test]
    fn headphones_leave_the_microphone_untouched_while_far_end_plays() {
        let processor = AudioProcessor::new();
        processor.configure(true, false);
        let mut random = 11u32;
        let (mut original, mut processed) = (0.0f64, 0.0f64);
        for frame in 0..1000 {
            // Far end and microphone have unrelated, slowly varying levels.
            let far_level = 0.05 + 0.05 * ((frame as f32) * 0.07).sin().abs();
            let mic_level = 0.05 + 0.05 * ((frame as f32) * 0.031 + 1.0).cos().abs();
            let mut playback = [0.0f32; 480];
            let mut mic = [0.0f32; 480];
            for (far, near) in playback.iter_mut().zip(&mut mic) {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *far = (random as f32 / u32::MAX as f32 - 0.5) * far_level;
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *near = (random as f32 / u32::MAX as f32 - 0.5) * mic_level;
            }
            processor.render(&playback);
            if frame >= 600 {
                original += mic.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
            processor.process_capture(&mut mic).unwrap();
            if frame >= 600 {
                processed += mic.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
        }
        assert!(
            processed > original * 0.9,
            "AEC attenuated an echo-free microphone: input={original}, output={processed}"
        );
    }

    #[test]
    fn incomplete_block_is_rejected_without_modification() {
        let processor = AudioProcessor::new();
        let mut frame = [0.25; 239];
        assert!(processor.process_capture(&mut frame).is_err());
        assert_eq!(frame, [0.25; 239]);
    }

    #[test]
    fn aec3_reduces_a_delayed_playback_echo_after_convergence() {
        let processor = AudioProcessor::new();
        processor.configure(true, false);
        processor.set_delay_ms(80);
        let mut history = VecDeque::from(vec![0.0; 24_000 * 80 / 1000]);
        let mut random = 7u32;
        let mut filtered = 0.0f32;
        let (mut original_energy, mut processed_energy) = (0.0f64, 0.0f64);
        for frame in 0..500 {
            let mut playback = [0.0; 480];
            let mut captured = [0.0; 480];
            for (render, capture) in playback.iter_mut().zip(&mut captured) {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (random as f32 / u32::MAX as f32 - 0.5) * 0.3;
                filtered = filtered * 0.6 + noise * 0.4;
                *render = filtered;
                history.push_back(*render);
                *capture = history.pop_front().unwrap() * 0.6;
            }
            processor.render(&playback);
            if frame >= 400 {
                original_energy += captured
                    .iter()
                    .map(|sample| f64::from(*sample).powi(2))
                    .sum::<f64>();
            }
            processor.process_capture(&mut captured).unwrap();
            if frame >= 400 {
                processed_energy += captured
                    .iter()
                    .map(|sample| f64::from(*sample).powi(2))
                    .sum::<f64>();
            }
        }
        assert!(
            processed_energy < original_energy * 0.5,
            "AEC did not reduce the known echo: input={original_energy}, output={processed_energy}"
        );
    }
}
