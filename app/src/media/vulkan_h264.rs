//! H.264 encoding through Vulkan Video (`gpu-video`).
//!
//! Media Foundation exposes only one hardware MFT on some AMD drivers and that
//! MFT stops accepting input after a few seconds of 1080p60 motion. Vulkan
//! Video reaches the same GPU through a different driver path and keeps up with
//! the requested cadence. Frames arrive here as packed NV12, converted on the
//! GPU from the captured D3D11 texture.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpu_video::parameters::{
    EncoderParametersH264, ProfileH264, RateControl, VideoAdapterDescriptor, VideoDeviceDescriptor,
    VideoInstanceDescriptor, VideoParameters,
};
use gpu_video::{BytesEncoderH264, InputFrame, RawFrameRef, VideoInstance};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};

use super::codec::PixelFormat;
use super::hw_h264::{pixels_to_nv12, GpuConvert};
use super::wgc::GpuFrame;

type ChunkQueue = Arc<Mutex<Vec<(u64, Vec<u8>, bool)>>>;

/// Build the Vulkan Video encoder without a D3D11 device. Also used by the
/// round-trip test, which feeds NV12 bytes straight from CPU memory.
fn create_bytes_encoder(
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
) -> Result<(BytesEncoderH264, ChunkQueue, String), String> {
    if width < 2 || height < 2 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err("Dimensões inválidas para o encoder Vulkan".into());
    }
    let instance = VideoInstance::new(&VideoInstanceDescriptor::default())
        .map_err(|e| format!("instância Vulkan Video: {e}"))?;
    let adapter = instance
        .create_adapter(&VideoAdapterDescriptor {
            supports_encoding: true,
            ..Default::default()
        })
        .map_err(|e| format!("adaptador Vulkan Video: {e}"))?;
    let name = format!("Vulkan Video ({})", adapter.info().name);
    let video_device = adapter
        .create_device(&VideoDeviceDescriptor::default())
        .map_err(|e| format!("dispositivo Vulkan Video: {e}"))?;
    let mut output_parameters = video_device
        .encoder_output_parameters_h264_low_latency(RateControl::VariableBitrate {
            average_bitrate: u64::from(bitrate),
            max_bitrate: u64::from(bitrate).saturating_mul(120) / 100,
            virtual_buffer_size: Duration::from_millis(500),
        })
        .map_err(|e| format!("parâmetros H.264 Vulkan: {e}"))?;
    // Keep the bitstream inside the receiver's bounded Baseline validator.
    output_parameters.profile = ProfileH264::Baseline;
    output_parameters.max_references = NonZeroU32::new(1);
    let params = EncoderParametersH264 {
        output_parameters,
        input_parameters: VideoParameters {
            width: NonZeroU32::new(width).ok_or("largura inválida")?,
            height: NonZeroU32::new(height).ok_or("altura inválida")?,
            target_framerate: fps.max(1).into(),
        },
        max_in_flight_submissions: None,
    };
    let ready = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&ready);
    let encoder = video_device
        .create_bytes_encoder_h264(params, move |chunk| {
            sink.lock().expect("Vulkan Video callback mutex").push((
                chunk.pts.unwrap_or(0),
                chunk.data,
                chunk.is_keyframe,
            ));
        })
        .map_err(|e| format!("encoder H.264 Vulkan: {e}"))?;
    Ok((encoder, ready, name))
}

/// A hardware H.264 encoder backed by Vulkan Video.
pub struct VulkanEncoder {
    encoder: BytesEncoderH264,
    name: String,
    width: u32,
    height: u32,
    pts: u64,
    /// Chunks arrive on the library's callback thread; collect them briefly
    /// and order by presentation timestamp before handing them to the app.
    ready: ChunkQueue,
    pending: VecDeque<(Vec<u8>, bool)>,
    /// Converts captured D3D11 pictures to NV12 without a full RGB staging copy.
    convert: GpuConvert,
    nv12: Vec<u8>,
}

impl VulkanEncoder {
    pub fn new(
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> Result<Self, String> {
        let (encoder, ready, name) = create_bytes_encoder(width, height, fps, bitrate)?;
        Ok(Self {
            encoder,
            name,
            width,
            height,
            pts: 0,
            ready,
            pending: VecDeque::new(),
            convert: GpuConvert::new(device, context, width, height, fps)?,
            nv12: Vec::new(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The Vulkan encoder is created at the current target bitrate; the
    /// bitrate controller's later nudges do not require a new video session.
    pub fn set_bitrate(&mut self, _bitrate: u32) -> Result<(), String> {
        Ok(())
    }

    /// Encode one NV12 picture. Returns the next finished access unit, or
    /// `None` while the asynchronous encoder still has a frame in flight.
    pub fn encode_nv12(
        &mut self,
        nv12: &[u8],
        force_keyframe: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        let expected = self.width as usize * self.height as usize * 3 / 2;
        if nv12.len() != expected {
            return Err("Tamanho de quadro NV12 inesperado para o encoder Vulkan".into());
        }
        let pts = self.pts;
        self.pts = self.pts.wrapping_add(1);
        self.encoder
            .encode(
                &InputFrame {
                    data: RawFrameRef {
                        frame: nv12,
                        width: self.width,
                        height: self.height,
                    },
                    pts: Some(pts),
                },
                force_keyframe,
            )
            .map_err(|e| format!("encode Vulkan Video: {e}"))?;
        let mut chunks =
            std::mem::take(&mut *self.ready.lock().expect("Vulkan Video callback mutex"));
        chunks.sort_by_key(|(pts, _, _)| *pts);
        self.pending.extend(
            chunks
                .into_iter()
                .map(|(_, data, keyframe)| (data, keyframe)),
        );
        Ok(self.pending.pop_front())
    }

    /// CPU-pixel entry point for the non-texture capture path.
    pub fn encode(
        &mut self,
        pixels: &[u8],
        format: PixelFormat,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        pixels_to_nv12(
            pixels,
            self.width as usize,
            self.height as usize,
            &mut self.nv12,
            format,
        );
        self.encode_owned_nv12(force)
    }

    /// GPU texture entry point used by screen sharing.
    pub fn encode_texture(
        &mut self,
        frame: &GpuFrame,
        force: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        self.nv12 = self.convert.read_nv12(frame, self.width, self.height)?;
        self.encode_owned_nv12(force)
    }

    fn encode_owned_nv12(&mut self, force: bool) -> Result<Option<(Vec<u8>, bool)>, String> {
        let nv12 = std::mem::take(&mut self.nv12);
        let result = self.encode_nv12(&nv12, force);
        self.nv12 = nv12;
        result
    }

    pub fn take_waits(&mut self) -> (f32, f32) {
        (0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::temporal::TemporalDecoder;
    use crate::protocol::EncodedVideoFrame;

    /// The standard receiver decoder must accept frames from this backend,
    /// including its CABAC PPS. Skip only when the machine has no Vulkan
    /// Video encoder at all.
    #[test]
    fn vulkan_frames_decode_with_the_standard_receiver() {
        const W: u32 = 640;
        const H: u32 = 360;
        let (mut encoder, ready, _) = match create_bytes_encoder(W, H, 30, 2_000_000) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Vulkan Video indisponível: {error}");
                return;
            }
        };
        let mut decoder = TemporalDecoder::new().unwrap();
        let mut nv12 = vec![0u8; (W * H * 3 / 2) as usize];
        let mut decoded = 0;
        let mut sequence = 0;
        for i in 0..30u32 {
            for (n, pixel) in nv12[..(W * H) as usize].iter_mut().enumerate() {
                *pixel = ((n + i as usize * 31) & 0xff) as u8;
            }
            encoder
                .encode(
                    &InputFrame {
                        data: RawFrameRef {
                            frame: &nv12,
                            width: W,
                            height: H,
                        },
                        pts: Some(i as u64),
                    },
                    i == 0,
                )
                .unwrap();
            let mut chunks = std::mem::take(&mut *ready.lock().unwrap());
            chunks.sort_by_key(|(pts, _, _)| *pts);
            for (_, data, keyframe) in chunks {
                sequence += 1;
                let frame = EncodedVideoFrame {
                    seq: sequence,
                    width: W,
                    height: H,
                    keyframe,
                    data,
                };
                if decoder.decode(&frame).unwrap().is_some() {
                    decoded += 1;
                }
            }
        }
        eprintln!("Vulkan Video decoded {decoded} frames");
        assert!(decoded > 0, "receiver cannot decode Vulkan Video output");
    }
}
