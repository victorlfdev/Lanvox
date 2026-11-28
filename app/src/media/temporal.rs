//! H.264 for motion video, with a deliberately restricted baseline bitstream.
//! Parameter sets are bounded before entering the native decoder.
//! Encoding tries a GPU encoder first (Media Foundation, Windows) and only
//! keeps it if its first IDR passes our own strict decoder; otherwise, or on
//! any later error, OpenH264 on the CPU takes over. Set
//! `LANVOX_H264_SOFTWARE=1` to force the CPU path.
//! OpenH264 is Cisco's own binary (see `openh264_runtime`); without it both
//! encoder and decoder refuse to start and video uses JPEG instead.

use super::codec::PixelFormat;
#[cfg(windows)]
use super::wgc::GpuFrame;
use crate::protocol::{picture_bytes, EncodedVideoFrame, MAX_VIDEO_PIXELS, MAX_VIDEO_SIDE};
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, Profile,
    RateControlMode, UsageType,
};
use openh264::formats::{BgraSliceU8, RgbaSliceU8, YUVBuffer, YUVSource};
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};

/// Common interface for hardware backends. Vulkan Video is preferred on
/// Windows; Media Foundation remains as the fallback for drivers without
/// Vulkan video encoding.
trait HwBackend {
    fn name(&self) -> String;
    fn size(&self) -> (u32, u32);
    fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String>;
    fn encode(
        &mut self,
        pixels: &[u8],
        format: PixelFormat,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String>;
    #[cfg(windows)]
    fn encode_texture(
        &mut self,
        frame: &GpuFrame,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String>;
    fn take_waits(&mut self) -> (f32, f32);
}

impl HwBackend for super::hw_h264::HwEncoder {
    fn name(&self) -> String {
        self.name().to_string()
    }
    fn size(&self) -> (u32, u32) {
        self.size()
    }
    fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        self.set_bitrate(bitrate)
    }
    fn encode(
        &mut self,
        pixels: &[u8],
        format: PixelFormat,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        self.encode(pixels, format, force)
    }
    #[cfg(windows)]
    fn encode_texture(
        &mut self,
        frame: &GpuFrame,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        self.encode_texture(frame, force)
    }
    fn take_waits(&mut self) -> (f32, f32) {
        self.take_waits()
    }
}

#[cfg(windows)]
impl HwBackend for super::vulkan_h264::VulkanEncoder {
    fn name(&self) -> String {
        self.name().to_string()
    }
    fn size(&self) -> (u32, u32) {
        self.size()
    }
    fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
        self.set_bitrate(bitrate)
    }
    fn encode(
        &mut self,
        pixels: &[u8],
        format: PixelFormat,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        self.encode(pixels, format, force)
    }
    fn encode_texture(
        &mut self,
        frame: &GpuFrame,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        self.encode_texture(frame, force)
    }
    fn take_waits(&mut self) -> (f32, f32) {
        self.take_waits()
    }
}

enum HwState {
    Untried,
    /// The bool is whether the first IDR was checked against our own decoder.
    Active(Box<dyn HwBackend>, bool),
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
    cpu_bitrate: u32,
    hw: HwState,
    /// Accumulated GPU encoder waits (NeedInput, output) since last read, in ms.
    hw_need_ms: f32,
    hw_out_ms: f32,
    #[cfg(windows)]
    gpu_device: Option<(ID3D11Device, ID3D11DeviceContext)>,
}

impl TemporalEncoder {
    /// "GPU (name)" or "CPU", for the stats overlay.
    pub fn backend(&self) -> String {
        match &self.hw {
            HwState::Active(hw, _) => format!("GPU: {}", hw.name()),
            _ => "CPU".into(),
        }
    }

    #[cfg(windows)]
    pub fn has_gpu_texture(&self) -> bool {
        self.gpu_device.is_some()
    }

    /// GPU encoder event waits since the last call, in milliseconds.
    #[cfg(test)]
    pub fn take_hw_waits(&mut self) -> (f32, f32) {
        (
            std::mem::take(&mut self.hw_need_ms),
            std::mem::take(&mut self.hw_out_ms),
        )
    }

    #[cfg(windows)]
    pub fn new_with_device(
        bitrate: u32,
        fps: u32,
        screen: bool,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Self, String> {
        let mut encoder = Self::new(bitrate, fps, screen)?;
        encoder.gpu_device = Some((device.clone(), context.clone()));
        Ok(encoder)
    }

    fn create_hardware(&self, width: u32, height: u32) -> Result<Box<dyn HwBackend>, String> {
        #[cfg(windows)]
        if let Some((device, context)) = &self.gpu_device {
            // Vulkan Video uses a separate driver path from Media Foundation.
            // Prefer it: the MF MFT on some AMD drivers stalls after a few
            // seconds of 1080p60 motion. Keep the MFT as the fallback.
            match super::vulkan_h264::VulkanEncoder::new(
                width,
                height,
                self.fps,
                self.bitrate,
                device,
                context,
            ) {
                Ok(encoder) => return Ok(Box::new(encoder)),
                Err(error) => {
                    eprintln!("Vulkan Video indisponível, usando Media Foundation: {error}")
                }
            }
            return Ok(Box::new(super::hw_h264::HwEncoder::new_with_device(
                width,
                height,
                self.fps,
                self.bitrate,
                device,
                context,
            )?));
        }
        Ok(Box::new(super::hw_h264::HwEncoder::new(
            width,
            height,
            self.fps,
            self.bitrate,
        )?))
    }

    /// GPU texture variant of the hardware encoder path.
    #[cfg(windows)]
    pub fn encode_texture(
        &mut self,
        frame: &GpuFrame,
        force: bool,
    ) -> Result<Option<EncodedVideoFrame>, String> {
        if !frame.width.is_multiple_of(2) || !frame.height.is_multiple_of(2) {
            return Err("Dimensões inválidas para vídeo H.264".into());
        }
        if matches!(self.hw, HwState::Active(ref hw, _) if hw.size() != (frame.width, frame.height))
        {
            self.hw = HwState::Untried;
        }
        if matches!(self.hw, HwState::Untried) {
            self.hw = match self.create_hardware(frame.width, frame.height) {
                Ok(hw) => HwState::Active(hw, false),
                Err(error) => {
                    eprintln!("H.264 por GPU indisponível, usando CPU: {error}");
                    HwState::Off
                }
            };
        }
        if crate::branding::env_flag("LANVOX_H264_SOFTWARE") {
            self.hw = HwState::Off;
        }
        let result = self.encode_hw_common(frame.width, frame.height, |hw| {
            hw.encode_texture(frame, force)
        });
        match result {
            Some(result) => result,
            None => Ok(None),
        }
    }

    fn encode_hardware(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        force: bool,
    ) -> Option<Result<Option<EncodedVideoFrame>, String>> {
        if crate::branding::env_flag("LANVOX_H264_SOFTWARE") {
            self.hw = HwState::Off;
        }
        if matches!(self.hw, HwState::Active(ref hw, _) if hw.size() != (width, height)) {
            self.hw = HwState::Untried;
        }
        if matches!(self.hw, HwState::Untried) {
            self.hw = match self.create_hardware(width, height) {
                Ok(hw) => HwState::Active(hw, false),
                Err(error) => {
                    eprintln!("H.264 por GPU indisponível, usando CPU: {error}");
                    HwState::Off
                }
            };
        }
        self.encode_hw_common(width, height, |hw| hw.encode(pixels, format, force))
    }

    fn encode_hw_common<F>(
        &mut self,
        width: u32,
        height: u32,
        encode: F,
    ) -> Option<Result<Option<EncodedVideoFrame>, String>>
    where
        F: FnOnce(&mut dyn HwBackend) -> Result<Option<(Vec<u8>, bool)>, String>,
    {
        let HwState::Active(hw, verified) = &mut self.hw else {
            return None;
        };
        let outcome = match hw
            .set_bitrate(self.bitrate)
            .and_then(|()| encode(hw.as_mut()))
        {
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
                    let valid = if *verified {
                        true
                    } else if keyframe {
                        match TemporalDecoder::new().and_then(|mut decoder| decoder.decode(&frame))
                        {
                            Ok(Some(_)) => true,
                            Ok(None) => false,
                            Err(error) => {
                                eprintln!("Encoder de GPU: IDR rejeitado ({error})");
                                false
                            }
                        }
                    } else {
                        false
                    };
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
        let (need_ms, out_ms) = hw.take_waits();
        self.hw_need_ms += need_ms;
        self.hw_out_ms += out_ms;
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

    /// Applied on the next encode, in place on GPU and initialized CPU encoders.
    pub fn set_bitrate(&mut self, bitrate: u32) {
        self.bitrate = bitrate.clamp(128_000, 80_000_000);
    }

    pub fn new(bitrate: u32, fps: u32, screen: bool) -> Result<Self, String> {
        let encoder = Self::build_encoder(bitrate, fps, screen)?;
        Ok(Self {
            encoder,
            sequence: 0,
            bitrate: bitrate.clamp(128_000, 80_000_000),
            fps: fps.clamp(1, 60),
            screen,
            cpu_size: None,
            cpu_bitrate: bitrate.clamp(128_000, 40_000_000),
            hw: HwState::Untried,
            hw_need_ms: 0.0,
            hw_out_ms: 0.0,
            #[cfg(windows)]
            gpu_device: None,
        })
    }

    pub fn encode(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        force: bool,
    ) -> Result<Option<EncodedVideoFrame>, String> {
        self.encode_format(rgba, width, height, PixelFormat::Rgba, force)
    }

    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        force: bool,
    ) -> Result<Option<EncodedVideoFrame>, String> {
        self.encode_format(bgra, width, height, PixelFormat::Bgra, force)
    }

    fn encode_format(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        force: bool,
    ) -> Result<Option<EncodedVideoFrame>, String> {
        if !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || picture_bytes(width, height) != Some(pixels.len())
        {
            return Err("Dimensões inválidas para vídeo H.264".into());
        }
        let was_hardware = matches!(self.hw, HwState::Active(..));
        if let Some(result) = self.encode_hardware(pixels, width, height, format, force) {
            return result;
        }
        // A switch from GPU to CPU mid-stream must start with a fresh IDR.
        let mut force = force || was_hardware;
        if self.cpu_size != Some((width, height)) {
            if self.cpu_size.is_some() || self.cpu_bitrate != self.bitrate {
                self.encoder = Self::build_encoder(self.bitrate, self.fps, self.screen)?;
            }
            self.cpu_bitrate = self.bitrate;
            self.cpu_size = Some((width, height));
            force = true;
        }
        if self.cpu_bitrate != self.bitrate {
            use openh264_sys2::{
                SBitrateInfo, ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SPATIAL_LAYER_0,
                SPATIAL_LAYER_ALL,
            };
            let mut info = SBitrateInfo {
                iLayer: SPATIAL_LAYER_ALL,
                iBitrate: self.bitrate as i32,
            };
            let options = if self.bitrate > self.cpu_bitrate {
                [ENCODER_OPTION_MAX_BITRATE, ENCODER_OPTION_BITRATE]
            } else {
                [ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE]
            };
            for option in options {
                // ALL changes only the global maximum, leaving the spatial
                // layer capped at its original bitrate. Update layer zero.
                info.iLayer = if option == ENCODER_OPTION_MAX_BITRATE {
                    SPATIAL_LAYER_0
                } else {
                    SPATIAL_LAYER_ALL
                };
                // The encoder is initialized, exclusively owned by this worker,
                // and SetOption reads the ABI-matched SBitrateInfo synchronously.
                let result = unsafe {
                    self.encoder
                        .raw_api()
                        .set_option(option, (&mut info as *mut SBitrateInfo).cast())
                };
                if result != 0 {
                    return Err(format!("OpenH264 recusou ajuste de bitrate ({result})"));
                }
            }
            self.cpu_bitrate = self.bitrate;
        }
        if force {
            self.encoder.force_intra_frame();
        }
        let dimensions = (width as usize, height as usize);
        let yuv = match format {
            PixelFormat::Rgba => YUVBuffer::from_rgba8_source(RgbaSliceU8::new(pixels, dimensions)),
            PixelFormat::Bgra => YUVBuffer::from_bgra8_source(BgraSliceU8::new(pixels, dimensions)),
        };
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
    // No FMO/ASO or large reference arrays. CABAC is a standard H.264 entropy
    // mode and is accepted: the Vulkan hardware encoder emits it even with a
    // Baseline SPS, and receivers must decode those frames. SPS dimensions and
    // reference counts stay bounded before the native decoder is called.
    let pps_id = bits.ue();
    let sps_id = bits.ue();
    let _entropy_flag = bits.bits(1);
    matches!(
        (
            pps_id,
            sps_id,
            bits.bits(1),
            bits.ue(),
            bits.ue(),
            bits.ue()
        ),
        (
            Some(0..=255),
            Some(0..=31),
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
    fn vulkan_cabac_pps_is_accepted_with_bounded_references() {
        // The Vulkan encoder writes CABAC even with a Baseline SPS; receivers
        // must accept it while still rejecting unbounded reference arrays.
        let pps = [0x68, 0xee, 0x3c, 0x80];
        assert!(valid_pps(&pps));
        assert!(!valid_pps(&[0x68]));
    }

    #[test]
    fn live_bitrate_changes_preserve_sequence_and_decode_on_cpu_and_available_gpu() {
        for software in [true, false] {
            let mut encoder = TemporalEncoder::new(2_000_000, 30, false).unwrap();
            if software {
                encoder.hw = HwState::Off;
            }
            let mut decoder = TemporalDecoder::new().unwrap();
            let (w, h) = (640u32, 360u32);
            let mut pixels = vec![255u8; (w * h * 4) as usize];
            let mut last_seq = 0u32;
            let mut decoded = 0;
            for i in 0..180u32 {
                let requested = [2_000_000, 1_000_000, 2_800_000, 1_000_000][(i / 45) as usize];
                encoder.set_bitrate(requested);
                for (n, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let x = n as u32 % w;
                    let y = n as u32 / w;
                    *pixel = [
                        ((x + i * 3) % 251) as u8,
                        ((y + i) % 251) as u8,
                        ((x / 8 + y / 8) % 251) as u8,
                        255,
                    ];
                }
                if let Some(frame) = encoder.encode(&pixels, w, h, i == 0).unwrap() {
                    assert_eq!(frame.seq, last_seq.wrapping_add(1));
                    last_seq = frame.seq;
                    assert!(decoder.decode(&frame).unwrap().is_some(), "frame {i}");
                    decoded += 1;
                }
                if matches!(encoder.hw, HwState::Off) {
                    use openh264_sys2::{SBitrateInfo, ENCODER_OPTION_BITRATE, SPATIAL_LAYER_ALL};
                    let mut actual = SBitrateInfo {
                        iLayer: SPATIAL_LAYER_ALL,
                        iBitrate: 0,
                    };
                    let result = unsafe {
                        encoder.encoder.raw_api().get_option(
                            ENCODER_OPTION_BITRATE,
                            (&mut actual as *mut SBitrateInfo).cast(),
                        )
                    };
                    assert_eq!(result, 0);
                    assert_eq!(actual.iBitrate as u32, requested);
                }
            }
            eprintln!(
                "live bitrate changes {}: {decoded}/180 decoded",
                encoder.backend()
            );
            assert!(decoded >= 170);
        }
    }

    #[test]
    #[ignore = "manual 1080p60 encoder benchmark; uses the available GPU or CPU"]
    fn benchmark_1080p_motion_encode() {
        let (w, h) = (1920u32, 1080u32);
        let mut encoder = TemporalEncoder::new(12_000_000, 60, true).unwrap();
        let mut decoder = TemporalDecoder::new().unwrap();
        let mut pixels = vec![255; (w * h * 4) as usize];
        let mut timings = Vec::new();
        let mut bytes = [0usize; 4];
        let mut decoded = 0;
        for i in 0..120u32 {
            let phase = (i / 30) as usize;
            encoder.set_bitrate([6_000_000, 8_000_000, 12_000_000, 6_000_000][phase]);
            for (n, px) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = n as u32 % w;
                let y = n as u32 / w;
                *px = [
                    ((x + i * 8) % 256) as u8,
                    ((y + i * 3) % 256) as u8,
                    ((x / 16 + y / 16 + i) % 256) as u8,
                    255,
                ];
            }
            let start = std::time::Instant::now();
            let frame = encoder.encode(&pixels, w, h, i == 0).unwrap();
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
            if let Some(frame) = frame {
                bytes[phase] += frame.data.len();
                if decoder.decode(&frame).unwrap().is_some() {
                    decoded += 1;
                }
            }
        }
        timings.sort_by(f64::total_cmp);
        eprintln!("1080p60 {}: encode mean {:.2}ms p50 {:.2}ms p95 {:.2}ms max {:.2}ms, {decoded}/120 decoded", encoder.backend(), timings.iter().sum::<f64>()/timings.len() as f64, timings[60], timings[114], timings[119]);
        eprintln!("1080p60 dynamic targets 6/8/12/6 Mb/s: bytes per 30-frame phase {bytes:?}");
        assert_eq!(decoded, 120);
    }

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
