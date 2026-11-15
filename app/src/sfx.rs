//! Short synthesized notification sounds, played on their own small output
//! stream so the voice mixer is never touched.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::model::{ChannelId, UserId};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sfx {
    Join,
    Leave,
    Ring,
    Message,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
}

/// (frequency Hz, duration ms); a frequency of 0 is silence.
fn notes(kind: Sfx) -> &'static [(f32, u32)] {
    match kind {
        Sfx::Join => &[(660.0, 90), (880.0, 140)],
        Sfx::Leave => &[(660.0, 90), (440.0, 150)],
        Sfx::Ring => &[
            (880.0, 150),
            (660.0, 150),
            (0.0, 80),
            (880.0, 150),
            (660.0, 150),
            (0.0, 80),
            (880.0, 150),
            (660.0, 200),
        ],
        Sfx::Message => &[(1175.0, 70), (1568.0, 110)],
        Sfx::Mute => &[(520.0, 70), (390.0, 110)],
        Sfx::Unmute => &[(390.0, 70), (520.0, 110)],
        Sfx::Deafen => &[(520.0, 60), (390.0, 60), (290.0, 110)],
        Sfx::Undeafen => &[(290.0, 60), (390.0, 60), (520.0, 110)],
    }
}

fn synthesize(kind: Sfx, rate: u32, volume: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let amp = 0.35 * volume;
    for &(freq, ms) in notes(kind) {
        let count = (rate as u64 * ms as u64 / 1000) as usize;
        let fade = (rate as usize / 200).max(1); // 5 ms
        for i in 0..count {
            let t = i as f32 / rate as f32;
            let tone = if freq > 0.0 {
                let phase = std::f32::consts::TAU * freq * t;
                phase.sin() + 0.25 * (phase * 2.0).sin()
            } else {
                0.0
            };
            let attack = (i as f32 / fade as f32).min(1.0);
            let release = ((count - i) as f32 / (fade * 4) as f32).min(1.0);
            out.push(tone * amp * attack * release);
        }
    }
    out
}

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

pub fn play(kind: Sfx, settings: &Settings) {
    if !settings.sounds || settings.sounds_volume <= 0.0 {
        return;
    }
    if ACTIVE.load(Ordering::Relaxed) >= 4 {
        return;
    }
    let volume = settings.sounds_volume.clamp(0.0, 1.0);
    let device_name = settings.output_device.clone();
    ACTIVE.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("sfx".into())
        .spawn(move || {
            let _ = run(kind, volume, device_name);
            ACTIVE.fetch_sub(1, Ordering::Relaxed);
        });
    if spawned.is_err() {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

fn run(kind: Sfx, volume: f32, device_name: Option<String>) -> Result<(), String> {
    let host = cpal::default_host();
    let device = device_name
        .and_then(|name| {
            host.output_devices()
                .ok()?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
        })
        .or_else(|| host.default_output_device())
        .ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let samples = synthesize(kind, rate, volume);
    let duration = Duration::from_millis(samples.len() as u64 * 1000 / rate as u64 + 120);
    let cfg: cpal::StreamConfig = config.clone().into();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, &cfg, samples, channels),
        cpal::SampleFormat::I16 => build::<i16>(&device, &cfg, samples, channels),
        cpal::SampleFormat::U16 => build::<u16>(&device, &cfg, samples, channels),
        other => Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(duration);
    Ok(())
}

fn build<T: cpal::SizedSample + cpal::FromSample<f32> + Send + 'static>(
    device: &cpal::Device,
    cfg: &cpal::StreamConfig,
    samples: Vec<f32>,
    channels: usize,
) -> Result<cpal::Stream, String> {
    let mut index = 0usize;
    device
        .build_output_stream(
            cfg,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels) {
                    let v = samples.get(index).copied().unwrap_or(0.0);
                    index += 1;
                    for sample in frame {
                        *sample = T::from_sample(v);
                    }
                }
            },
            |e| eprintln!("sfx error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// Turns changes in who is in my voice channel into join/leave sounds and
/// repeats the ring while a private call is waiting.
#[derive(Default)]
pub struct Watcher {
    channel: Option<ChannelId>,
    members: HashSet<UserId>,
    last_ring: Option<Instant>,
}

impl Watcher {
    pub fn members(
        &mut self,
        me: UserId,
        channel: Option<ChannelId>,
        now: &[UserId],
    ) -> Option<Sfx> {
        let now: HashSet<UserId> = now.iter().copied().filter(|u| *u != me).collect();
        let same = channel.is_some() && channel == self.channel;
        let result = if same {
            if now.difference(&self.members).next().is_some() {
                Some(Sfx::Join)
            } else if self.members.difference(&now).next().is_some() {
                Some(Sfx::Leave)
            } else {
                None
            }
        } else {
            None
        };
        self.channel = channel;
        self.members = now;
        result
    }

    /// True when the ring should sound again.
    pub fn ring(&mut self, ringing: bool) -> bool {
        if !ringing {
            self.last_ring = None;
            return false;
        }
        match self.last_ring {
            Some(at) if at.elapsed() < Duration::from_millis(3500) => false,
            _ => {
                self.last_ring = Some(Instant::now());
                true
            }
        }
    }
}
