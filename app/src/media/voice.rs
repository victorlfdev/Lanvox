//! Opus voice packets and bounded adaptive playout. No remote/local wall clocks
//! are compared: jitter uses differences of sender and arrival intervals.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use opusic_c::{Application, Bitrate, Channels, Decoder, Encoder, InbandFec, SampleRate, Signal};

pub const FRAME: usize = 480;

pub struct VoiceEncoder(Encoder);

pub struct MusicEncoder(Encoder);

impl MusicEncoder {
    pub fn new() -> Result<Self, String> {
        let mut encoder = Encoder::new(Channels::Stereo, SampleRate::Hz48000, Application::Audio)
            .map_err(|e| format!("{e:?}"))?;
        encoder
            .set_bitrate(Bitrate::Value(128_000))
            .map_err(|e| format!("{e:?}"))?;
        encoder.set_vbr(true).map_err(|e| format!("{e:?}"))?;
        encoder
            .set_vbr_constraint(true)
            .map_err(|e| format!("{e:?}"))?;
        Ok(Self(encoder))
    }

    pub fn encode(&mut self, stereo: &[f32]) -> Result<Vec<u8>, String> {
        if stereo.len() != 1920 {
            return Err("Stream audio must contain 20 ms of stereo 48 kHz".into());
        }
        let mut encoded = [0; 1275];
        let len = self
            .0
            .encode_float_to_slice(stereo, &mut encoded)
            .map_err(|e| format!("{e:?}"))?;
        Ok(encoded[..len].to_vec())
    }
}

impl VoiceEncoder {
    pub fn new() -> Result<Self, String> {
        let mut encoder = Encoder::new(Channels::Mono, SampleRate::Hz24000, Application::Voip)
            .map_err(|e| format!("{e:?}"))?;
        encoder
            .set_bitrate(Bitrate::Value(48_000))
            .map_err(|e| format!("{e:?}"))?;
        encoder.set_vbr(true).map_err(|e| format!("{e:?}"))?;
        encoder
            .set_vbr_constraint(true)
            .map_err(|e| format!("{e:?}"))?;
        encoder
            .set_inband_fec(InbandFec::Mode1)
            .map_err(|e| format!("{e:?}"))?;
        encoder.set_packet_loss(10).map_err(|e| format!("{e:?}"))?;
        encoder.set_complexity(9).map_err(|e| format!("{e:?}"))?;
        encoder
            .set_signal(Signal::Voice)
            .map_err(|e| format!("{e:?}"))?;
        Ok(Self(encoder))
    }

    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, String> {
        if pcm.len() != FRAME {
            return Err("Voice frame must contain 20 ms".into());
        }
        let input: Vec<f32> = pcm
            .iter()
            .map(|sample| f32::from(*sample) / 32768.0)
            .collect();
        let mut encoded = [0; 1275];
        let len = self
            .0
            .encode_float_to_slice(&input, &mut encoded)
            .map_err(|e| format!("{e:?}"))?;
        Ok(encoded[..len].to_vec())
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct VoiceStats {
    pub jitter_ms: f32,
    pub target_ms: u32,
    pub lost: u64,
    pub late: u64,
}

pub struct JitterBuffer {
    decoder: Decoder,
    packets: HashMap<u32, Vec<u8>>,
    next: Option<u32>,
    first: Option<Instant>,
    last_arrival: Option<(Instant, u64, u32)>,
    started: bool,
    concealed: usize,
    stats: VoiceStats,
    frame_samples: usize,
    channels: usize,
}

impl JitterBuffer {
    pub fn new() -> Result<Self, String> {
        Self::with_format(Channels::Mono, SampleRate::Hz24000, FRAME, 1)
    }

    pub fn music() -> Result<Self, String> {
        Self::with_format(Channels::Stereo, SampleRate::Hz48000, 1920, 2)
    }

    fn with_format(
        channels: Channels,
        rate: SampleRate,
        frame_samples: usize,
        channel_count: usize,
    ) -> Result<Self, String> {
        Ok(Self {
            decoder: Decoder::new(channels, rate).map_err(|e| format!("{e:?}"))?,
            packets: HashMap::new(),
            next: None,
            first: None,
            last_arrival: None,
            started: false,
            concealed: 0,
            stats: VoiceStats {
                target_ms: 40,
                ..Default::default()
            },
            frame_samples,
            channels: channel_count,
        })
    }

    pub fn enqueue(&mut self, sequence: u32, sent_micros: u64, packet: &[u8]) {
        self.enqueue_at(sequence, sent_micros, packet, Instant::now());
    }

    fn enqueue_at(&mut self, sequence: u32, sent_micros: u64, packet: &[u8], now: Instant) {
        if packet.is_empty() || packet.len() > 1275 {
            return;
        }
        let newer = self
            .last_arrival
            .is_none_or(|(_, _, previous)| sequence.wrapping_sub(previous) as i32 > 0);
        if let Some((last, sent, _)) = self.last_arrival.filter(|_| newer) {
            let elapsed = now.saturating_duration_since(last);
            if elapsed > Duration::from_millis(400) || sent_micros < sent {
                self.packets.clear();
                self.started = false;
                self.next = Some(sequence);
                self.first = Some(now);
                self.concealed = 0;
                let _ = self.decoder.reset();
            } else if sent_micros > sent {
                let delta =
                    (elapsed.as_secs_f32() * 1000.0 - (sent_micros - sent) as f32 / 1000.0).abs();
                self.stats.jitter_ms += (delta - self.stats.jitter_ms) / 16.0;
                let frames = ((20.0 + self.stats.jitter_ms * 4.0) / 20.0)
                    .ceil()
                    .clamp(2.0, 8.0) as u32;
                self.stats.target_ms = frames * 20;
            }
        }
        if newer {
            self.last_arrival = Some((now, sent_micros, sequence));
        }
        self.first.get_or_insert(now);
        if let Some(next) = self.next {
            let distance = sequence.wrapping_sub(next) as i32;
            if distance < 0 {
                if !self.started && distance >= -8 {
                    self.next = Some(sequence);
                } else {
                    self.stats.late += 1;
                    return;
                }
            } else if distance > 15 {
                self.packets.clear();
                self.next = Some(sequence);
                self.started = false;
                self.first = Some(now);
            }
        } else {
            self.next = Some(sequence);
        }
        if self.packets.len() >= 16 && !self.packets.contains_key(&sequence) {
            self.stats.late += 1;
            return;
        }
        self.packets
            .entry(sequence)
            .or_insert_with(|| packet.to_vec());
    }

    pub fn pop(&mut self) -> Option<Vec<f32>> {
        if !self.started {
            if self.packets.is_empty()
                || (self.packets.len() * 20 < self.stats.target_ms as usize
                    && self.first.is_none_or(|first| {
                        first.elapsed().as_millis() < u128::from(self.stats.target_ms)
                    }))
            {
                return None;
            }
            self.started = true;
        }
        let next = self.next?;
        let packet = self.packets.remove(&next);
        let mut output = vec![0.0; self.frame_samples];
        let result = if let Some(packet) = packet {
            self.concealed = 0;
            self.decoder
                .decode_float_to_slice(&packet, &mut output, false)
        } else if let Some(following) = self.packets.get(&next.wrapping_add(1)) {
            self.stats.lost += 1;
            self.decoder
                .decode_float_to_slice(following, &mut output, true)
        } else {
            if self.concealed >= 3 {
                self.started = false;
                self.first = None;
                return None;
            }
            self.concealed += 1;
            self.stats.lost += 1;
            self.decoder.decode_float_to_slice(&[], &mut output, false)
        };
        self.next = Some(next.wrapping_add(1));
        match result {
            Ok(samples) if samples * self.channels == self.frame_samples => Some(output),
            _ => {
                self.stats.lost += 1;
                Some(vec![0.0; self.frame_samples])
            }
        }
    }

    pub fn stats(&self) -> VoiceStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_roundtrip_is_compressed_and_handles_reordering_loss_and_sequence_wrap() {
        let mut encoder = VoiceEncoder::new().unwrap();
        let pcm: Vec<i16> = (0..FRAME)
            .map(|i| {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 24_000.0).sin() * 10_000.0) as i16
            })
            .collect();
        let packet = encoder.encode(&pcm).unwrap();
        assert!(packet.len() < pcm.len() * 2);
        let mut jitter = JitterBuffer::new().unwrap();
        let now = Instant::now();
        jitter.enqueue_at(u32::MAX, 20_000, &packet, now);
        jitter.enqueue_at(u32::MAX - 1, 0, &packet, now);
        jitter.enqueue_at(1, 60_000, &packet, now + Duration::from_millis(60));
        assert_eq!(jitter.pop().unwrap().len(), FRAME);
        assert_eq!(jitter.pop().unwrap().len(), FRAME);
        assert_eq!(jitter.pop().unwrap().len(), FRAME);
        assert!(jitter.stats().lost > 0);
        assert_eq!(jitter.pop().unwrap().len(), FRAME);
        jitter.enqueue(u32::MAX - 1, 0, &packet);
        assert!(jitter.stats().late > 0);
    }

    #[test]
    fn jitter_target_increases_under_variable_arrival_without_unbounded_buffer() {
        let mut jitter = JitterBuffer::new().unwrap();
        let now = Instant::now();
        jitter.enqueue_at(0, 0, &[1], now);
        jitter.enqueue_at(1, 20_000, &[1], now + Duration::from_millis(130));
        assert!(jitter.stats().target_ms > 40);
        for sequence in 2..500 {
            jitter.enqueue_at(
                sequence,
                u64::from(sequence) * 20_000,
                &[1],
                now + Duration::from_millis(140),
            );
        }
        assert!(jitter.packets.len() <= 16);
        assert!(jitter.stats().target_ms <= 160);
    }

    #[test]
    fn stream_audio_preserves_stereo_channels_at_48khz() {
        let mut encoder = MusicEncoder::new().unwrap();
        let samples: Vec<f32> = (0..960)
            .flat_map(|index| {
                [
                    ((index as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5),
                    0.0,
                ]
            })
            .collect();
        let packet = encoder.encode(&samples).unwrap();
        assert!(packet.len() < 1275);
        let mut receiver = JitterBuffer::music().unwrap();
        receiver.enqueue(0, 0, &packet);
        receiver.enqueue(1, 20_000, &encoder.encode(&samples).unwrap());
        let decoded = receiver.pop().unwrap();
        assert_eq!(decoded.len(), 1920);
        let left: f32 = decoded
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| pair[0] * pair[0])
            .sum();
        let right: f32 = decoded
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| pair[1] * pair[1])
            .sum();
        assert!(left > 0.01 && left > right * 3.0);
    }
}
