//! H.264 for motion video, with a deliberately restricted baseline bitstream.
//! Parameter sets are bounded before entering the native decoder.
//! Encoding tries a GPU encoder first (Media Foundation, Windows) and only
//! keeps it if its first IDR passes our own strict decoder; otherwise, or on
//! any later error, OpenH264 on the CPU takes over. Set
//! `LANVOX_H264_SOFTWARE=1` to force the CPU path.
//! OpenH264 is Cisco's own binary (see `openh264_runtime`); without it both
//! encoder and decoder refuse to start and video uses JPEG instead.

use crate::protocol::{picture_bytes, EncodedVideoFrame, MAX_VIDEO_PIXELS, MAX_VIDEO_SIDE};
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};

enum HwState {
    Untried,
    /// The bool is whether the first IDR was checked against our own decoder.
    Active(super::hw_h264::HwEncoder, bool),
    Off,
}

/// Worker threads for the CPU encoder: half the cores (the game and the capture
/// need the rest), at least 2 and at most 4.
fn software_threads() -> u16 {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores / 2).clamp(2, 4) as u16
}

pub struct TemporalEncoder {
    encoder: Encoder,
    sequence: u32,
    bitrate: u32,
    fps: u32,
    screen: bool,
    /// Picture size the CPU encoder was last fed; a change needs a fresh encoder.
    cpu_size: Option<(u32, u32)>,
    hw: HwState,
}

impl TemporalEncoder {
    /// "GPU (name)" or "CPU", for the stats overlay.
    pub fn backend(&self) -> String {
        match &self.hw {
            HwState::Active(hw, _) => format!("GPU: {}", hw.name()),
            _ => "CPU".into(),
        }
    }

    fn encode_hardware(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        force: bool,
    ) -> Option<Result<Option<EncodedVideoFrame>, String>> {
        if crate::branding::env_flag("LANVOX_H264_SOFTWARE") {
            self.hw = HwState::Off;
        }
        if matches!(self.hw, HwState::Active(ref hw, _) if hw.size() != (width, height)) {
            self.hw = HwState::Untried;
        }
        if matches!(self.hw, HwState::Untried) {
            self.hw = match super::hw_h264::HwEncoder::new(width, height, self.fps, self.bitrate) {
                Ok(hw) => HwState::Active(hw, false),
                Err(error) => {
                    eprintln!("H.264 por GPU indisponível, usando CPU: {error}");
                    HwState::Off
                }
            };
        }
        let HwState::Active(hw, verified) = &mut self.hw else {
            return None;
        };
        let outcome = match hw.encode(rgba, force) {
            Ok(Some((data, keyframe))) => {
                if data.len() > 2 * 1024 * 1024 {
                    Err("Quadro H.264 excede o limite de 2 MiB".to_string())
                } else {
                    let frame = EncodedVideoFrame {
                        seq: self.sequence.wrapping_add(1),
                        width,
                        height,
                        keyframe,
                        data,
                    };
                    // The first picture must decode with the same strict
                    // validation the receivers apply, otherwise stay on CPU.
                    let valid = *verified
                        || (keyframe
                            && TemporalDecoder::new()
                                .and_then(|mut decoder| decoder.decode(&frame))
                                .is_ok_and(|picture| picture.is_some()));
                    if valid {
                        *verified = true;
                        Ok(Some(frame))
                    } else {
                        Err("Fluxo do encoder de GPU rejeitado pela validação".to_string())
                    }
                }
            }
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(frame) => {
                if let Some(frame) = &frame {
                    self.sequence = frame.seq;
                }
                Some(Ok(frame))
            }
            Err(error) => {
                eprintln!("Encoder de GPU falhou, voltando para CPU: {error}");
                self.hw = HwState::Off;
                None
            }
        }
    }

    fn build_encoder(bitrate: u32, fps: u32, screen: bool) -> Result<Encoder, String> {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate.clamp(128_000, 40_000_000)))
            .max_frame_rate(FrameRate::from_hz(fps.clamp(1, 60) as f32))
            .profile(Profile::Baseline)
            .usage_type(if screen {
                UsageType::ScreenContentRealTime
            } else {
                UsageType::CameraVideoRealTime
            })
            .rate_control_mode(RateControlMode::Bitrate)
            .intra_frame_period(IntraFramePeriod::from_num_frames(fps.clamp(1, 60) * 10))
            .num_threads(software_threads())
            .skip_frames(true);
        let api = super::openh264_runtime::api()
            .ok_or_else(|| "Codec H.264 da Cisco não instalado".to_string())?;
        Encoder::with_api_config(api, config).map_err(|error| error.to_string())
    }

    pub fn new(bitrate: u32, fps: u32, screen: bool) -> Result<Self, String> {
        let encoder = Self::build_encoder(bitrate, fps, screen)?;
        Ok(Self {
            encoder,
            sequence: 0,
            bitrate: bitrate.clamp(128_000, 40_000_000),
            fps: fps.clamp(1, 60),
            screen,
            cpu_size: None,
            hw: HwState::Untried,
        })
    }

    pub fn encode(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        force: bool,
    ) -> Result<Option<EncodedVideoFrame>, String> {
        if !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || picture_bytes(width, height) != Some(rgba.len())
        {
            return Err("Dimensões inválidas para vídeo H.264".into());
        }
        let was_hardware = matches!(self.hw, HwState::Active(..));
        if let Some(result) = self.encode_hardware(rgba, width, height, force) {
            return result;
        }
        // A switch from GPU to CPU mid-stream must start with a fresh IDR.
        let mut force = force || was_hardware;
        if self.cpu_size != Some((width, height)) {
            if self.cpu_size.is_some() {
                self.encoder = Self::build_encoder(self.bitrate, self.fps, self.screen)?;
            }
            self.cpu_size = Some((width, height));
            force = true;
        }
        if force {
            self.encoder.force_intra_frame();
        }
        let input = RgbaSliceU8::new(rgba, (width as usize, height as usize));
        let yuv = YUVBuffer::from_rgba8_source(input);
        let encoded = self
            .encoder
            .encode(&yuv)
            .map_err(|error| error.to_string())?;
        let keyframe = matches!(encoded.frame_type(), FrameType::IDR);
        let data = encoded.to_vec();
        if data.is_empty() || matches!(encoded.frame_type(), FrameType::Skip | FrameType::Invalid) {
            return Ok(None);
        }
        if data.len() > 2 * 1024 * 1024 {
            return Err("Quadro H.264 excede o limite de 2 MiB".into());
        }
        self.sequence = self.sequence.wrapping_add(1);
        Ok(Some(EncodedVideoFrame {
            seq: self.sequence,
            width,
            height,
            keyframe,
            data,
        }))
    }
}

struct Bits {
    data: Vec<u8>,
    bit: usize,
}

impl Bits {
    fn new(nal: &[u8]) -> Option<Self> {
        let mut data = Vec::with_capacity(nal.len());
        let mut zeros = 0;
        for (index, byte) in nal.iter().copied().enumerate() {
            if zeros >= 2 && byte == 3 {
                if nal.get(index + 1).is_none_or(|next| *next > 3) {
                    return None;
                }
                zeros = 0;
                continue;
            }
            data.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        Some(Self { data, bit: 0 })
    }
    fn bits(&mut self, count: usize) -> Option<u32> {
        if count > 32 || self.bit.checked_add(count)? > self.data.len() * 8 {
            return None;
        }
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u32::from((self.data[self.bit / 8] >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Some(value)
    }
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0usize;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 30 {
                return None;
            }
        }
        Some(((1u32 << zeros) - 1) + self.bits(zeros)?)
    }
}

fn sps_dimensions(nal: &[u8]) -> Option<(u32, u32)> {
    let mut bits = Bits::new(nal.get(1..)?)?;
    // The encoder emits progressive 8-bit YUV420 constrained baseline. Refuse
    // other profiles rather than allowing unbounded native SPS allocations.
    if bits.bits(8)? != 66 {
        return None;
    }
    bits.bits(8)?;
    bits.bits(8)?;
    if bits.ue()? > 31 || bits.ue()? > 12 {
        return None;
    }
    match bits.ue()? {
        0 => {
            if bits.ue()? > 12 {
                return None;
            }
        }
        1 => {
            bits.bits(1)?;
            bits.ue()?;
            bits.ue()?;
            let cycle = bits.ue()?;
            if cycle > 16 {
                return None;
            }
            for _ in 0..cycle {
                bits.ue()?;
            }
        }
        2 => {}
        _ => return None,
    }
    if bits.ue()? > 4 {
        return None;
    }
    bits.bits(1)?;
    let width = bits.ue()?.checked_add(1)?.checked_mul(16)?;
    let height = bits.ue()?.checked_add(1)?.checked_mul(16)?;
    if width > MAX_VIDEO_SIDE
        || height > MAX_VIDEO_SIDE
        || u64::from(width) * u64::from(height) > MAX_VIDEO_PIXELS + u64::from(MAX_VIDEO_SIDE) * 32
    {
        return None;
    }
    if bits.bits(1)? != 1 {
        return None;
    }
    bits.bits(1)?;
    let (mut cropped_w, mut cropped_h) = (width, height);
    if bits.bits(1)? == 1 {
        let (left, right, top, bottom) = (bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?);
        cropped_w = width.checked_sub(left.checked_add(right)?.checked_mul(2)?)?;
        cropped_h = height.checked_sub(top.checked_add(bottom)?.checked_mul(2)?)?;
    }
    picture_bytes(cropped_w, cropped_h)?;
    Some((cropped_w, cropped_h))
}

fn valid_pps(nal: &[u8]) -> bool {
    let Some(mut bits) = nal.get(1..).and_then(Bits::new) else {
        return false;
    };
    // No FMO/ASO, CABAC or large reference arrays are emitted by our baseline
    // encoder. Refuse those extensions before calling the native library.
    matches!(
        (
            bits.ue(),
            bits.ue(),
            bits.bits(1),
            bits.bits(1),
            bits.ue(),
            bits.ue(),
            bits.ue()
        ),
        (
            Some(0..=255),
            Some(0..=31),
            Some(0),
            Some(_),
            Some(0),
            Some(0..=3),
            Some(0..=3)
        )
    )
}

fn nals(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let mut starts = Vec::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        if bytes[index..].starts_with(&[0, 0, 0, 1]) {
            starts.push((index, index + 4));
            index += 4;
        } else if bytes[index..].starts_with(&[0, 0, 1]) {
            starts.push((index, index + 3));
            index += 3;
        } else {
            index += 1;
        }
        if starts.len() > 128 {
            return None;
        }
    }
    if starts.first()?.0 != 0 {
        return None;
    }
    Some(
        starts
            .iter()
            .enumerate()
            .map(|(i, (_, begin))| {
                &bytes[*begin
                    ..starts
                        .get(i + 1)
                        .map(|entry| entry.0)
                        .unwrap_or(bytes.len())]
            })
            .collect(),
    )
}

fn new_decoder() -> Result<Decoder, String> {
    let api = super::openh264_runtime::api()
        .ok_or_else(|| "Codec H.264 da Cisco não instalado".to_string())?;
    Decoder::with_api_config(api, DecoderConfig::new()).map_err(|error| error.to_string())
}

pub struct TemporalDecoder {
    decoder: Decoder,
    dimensions: Option<(u32, u32)>,
    sequence: Option<u32>,
}

impl TemporalDecoder {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            decoder: new_decoder()?,
            dimensions: None,
            sequence: None,
        })
    }

    pub fn decode(&mut self, frame: &EncodedVideoFrame) -> Result<Option<Vec<u8>>, String> {
        let invalid =
            || "Fluxo H.264 inválido ou sem quadro completo; solicitando recuperação".to_string();
        let bytes = picture_bytes(frame.width, frame.height).ok_or_else(invalid)?;
        if frame.data.is_empty() || frame.data.len() > 2 * 1024 * 1024 {
            return Err(invalid());
        }
        let units = nals(&frame.data).ok_or_else(invalid)?;
        let mut has_sps = false;
        let mut has_idr = false;
        for unit in units {
            let header = *unit.first().ok_or_else(invalid)?;
            if header & 0x80 != 0 {
                return Err(invalid());
            }
            match header & 31 {
                7 => {
                    if unit.len() > 1024
                        || sps_dimensions(unit) != Some((frame.width, frame.height))
                    {
                        return Err(invalid());
                    }
                    has_sps = true;
                }
                5 => has_idr = true,
                8 => {
                    if unit.len() > 1024 || !valid_pps(unit) {
                        return Err(invalid());
                    }
                }
                1 | 6 | 9 | 10 | 11 | 12 => {}
                _ => return Err(invalid()),
            }
        }
        if frame.keyframe {
            if !has_sps || !has_idr {
                return Err(invalid());
            }
            self.decoder = new_decoder()?;
            self.dimensions = Some((frame.width, frame.height));
        } else if self.dimensions != Some((frame.width, frame.height))
            || self
                .sequence
                .is_none_or(|last| last.wrapping_add(1) != frame.seq)
        {
            self.sequence = None;
            return Err(invalid());
        }
        let picture = self
            .decoder
            .decode(&frame.data)
            .map_err(|error| error.to_string())?;
        self.sequence = Some(frame.seq);
        if let Some(picture) = picture {
            if picture.dimensions() != (frame.width as usize, frame.height as usize) {
                self.sequence = None;
                return Err(invalid());
            }
            let mut output = vec![0; bytes];
            picture.write_rgba8(&mut output);
            Ok(Some(output))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_roundtrip_rejects_missing_delta_and_recovers_on_real_idr() {
        let mut encoder = TemporalEncoder::new(1_000_000, 30, true).unwrap();
        let mut decoder = TemporalDecoder::new().unwrap();
        let mut pixels = vec![120; 128 * 96 * 4];
        let first = encoder.encode(&pixels, 128, 96, true).unwrap().unwrap();
        assert!(first.keyframe);
        assert_eq!(decoder.decode(&first).unwrap().unwrap().len(), pixels.len());
        pixels[0] = 20;
        let _lost = encoder.encode(&pixels, 128, 96, false).unwrap();
        pixels[4] = 30;
        if let Some(next) = encoder.encode(&pixels, 128, 96, false).unwrap() {
            assert!(decoder.decode(&next).is_err());
        }
        let recovered = encoder.encode(&pixels, 128, 96, true).unwrap().unwrap();
        assert!(decoder.decode(&recovered).unwrap().is_some());
        let mut forged = recovered;
        forged.width = 4096;
        forged.height = 4096;
        assert!(decoder.decode(&forged).is_err());
    }

    /// Reports which backend ran; passes on machines without a GPU encoder
    /// because the encoder is required to fall back to the CPU.
    #[test]
    fn gpu_or_fallback_stream_decodes_and_recovers() {
        let (w, h) = (640u32, 360u32);
        let mut encoder = TemporalEncoder::new(2_000_000, 30, true).unwrap();
        let mut decoder = TemporalDecoder::new().unwrap();
        let mut pixels = vec![90u8; (w * h * 4) as usize];
        let mut decoded = 0;
        for i in 0..40u32 {
            for (n, px) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px[0] = ((n as u32 + i * 7) % 251) as u8;
                px[3] = 255;
            }
            if let Some(frame) = encoder.encode(&pixels, w, h, i == 0 || i == 20).unwrap() {
                assert_eq!(frame.keyframe, i == 0 || i == 20, "frame {i}");
                if decoder.decode(&frame).unwrap().is_some() {
                    decoded += 1;
                }
            }
        }
        eprintln!("backend: {} decoded: {decoded}", encoder.backend());
        assert!(decoded >= 30);
    }

    #[test]
    fn malformed_nals_and_exp_golomb_lengths_are_rejected() {
        assert!(nals(&[0, 1, 2]).is_none());
        assert!(sps_dimensions(&[0x67, 66, 0, 30, 0, 0, 0, 0, 0, 0]).is_none());
        let mut decoder = TemporalDecoder::new().unwrap();
        assert!(decoder
            .decode(&EncodedVideoFrame {
                seq: 1,
                width: 64,
                height: 64,
                keyframe: true,
                data: vec![0, 0, 1, 0x65]
            })
            .is_err());
    }
}
