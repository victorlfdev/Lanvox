//! GPU H.264 encoding through Windows Media Foundation hardware MFTs
//! (NVENC, Intel Quick Sync, AMD AMF). The bitstream is forced to constrained
//! baseline/CAVLC so the existing receiver validation and decoder accept it.
//!
//! Anything unexpected returns an error; the caller then falls back to the
//! OpenH264 software encoder, so a machine without a usable GPU encoder keeps
//! working exactly as before.

use super::codec::PixelFormat;

/// Converts tightly packed RGB channel layouts to NV12 (BT.601 limited range).
pub(crate) fn pixels_to_nv12(
    pixels: &[u8],
    w: usize,
    h: usize,
    out: &mut Vec<u8>,
    format: PixelFormat,
) {
    use rayon::prelude::*;
    let (red, green, blue) = match format {
        PixelFormat::Rgba => (0, 1, 2),
        PixelFormat::Bgra => (2, 1, 0),
    };
    out.resize(w * h * 3 / 2, 0);
    let (y_plane, uv_plane) = out.split_at_mut(w * h);
    y_plane
        .par_chunks_mut(2 * w)
        .zip(uv_plane.par_chunks_mut(w))
        .enumerate()
        .for_each(|(row, (y_rows, uv_row))| {
            let (y0, y1) = y_rows.split_at_mut(w);
            let src0 = &pixels[2 * row * w * 4..(2 * row + 1) * w * 4];
            let src1 = &pixels[(2 * row + 1) * w * 4..(2 * row + 2) * w * 4];
            for x in (0..w).step_by(2) {
                let (mut r_sum, mut g_sum, mut b_sum) = (0i32, 0i32, 0i32);
                for (src, y_row) in [(src0, &mut *y0), (src1, &mut *y1)] {
                    for dx in 0..2 {
                        let p = (x + dx) * 4;
                        let (r, g, b) = (
                            src[p + red] as i32,
                            src[p + green] as i32,
                            src[p + blue] as i32,
                        );
                        y_row[x + dx] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                        r_sum += r;
                        g_sum += g;
                        b_sum += b;
                    }
                }
                let (r, g, b) = ((r_sum + 2) >> 2, (g_sum + 2) >> 2, (b_sum + 2) >> 2);
                uv_row[x] = ((((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
                uv_row[x + 1] =
                    ((((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255)) as u8;
            }
        });
}

/// Returns true when the Annex-B stream contains a NAL unit of `kind`.
#[cfg_attr(not(windows), allow(dead_code))]
fn has_nal(data: &[u8], kind: u8) -> bool {
    data.windows(4)
        .any(|w| w[..3] == [0, 0, 1] && w[3] & 31 == kind)
}

#[cfg(windows)]
pub use imp::{GpuConvert, HwEncoder};

#[cfg(not(windows))]
pub struct HwEncoder;

#[cfg(not(windows))]
impl HwEncoder {
    pub fn new(_: u32, _: u32, _: u32, _: u32) -> Result<Self, String> {
        Err("Codificação por GPU só está disponível no Windows".into())
    }
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }
    pub fn name(&self) -> &str {
        ""
    }
    pub fn set_bitrate(&mut self, _: u32) -> Result<(), String> {
        Err("indisponível".into())
    }
    pub fn encode(
        &mut self,
        _: &[u8],
        _: PixelFormat,
        _: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, String> {
        Err("indisponível".into())
    }
}

#[cfg(windows)]
mod imp {
    use super::{has_nal, pixels_to_nv12, PixelFormat};
    use std::collections::VecDeque;
    use std::mem::ManuallyDrop;
    use std::time::{Duration, Instant};
    use windows::core::{Interface, GUID};
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
        ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorOutputView,
        D3D11_BIND_RENDER_TARGET, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
        D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CAPS,
        D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
        D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
        D3D11_VPOV_DIMENSION_TEXTURE2D,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
    use windows::Win32::System::Variant::VARIANT;

    use super::super::wgc::GpuFrame;

    /// Process-wide Media Foundation lifetime (MFStartup is reference counted).
    struct MfRuntime;
    impl MfRuntime {
        fn start() -> Result<Self, String> {
            unsafe {
                // Ignore RPC_E_CHANGED_MODE: the thread may already be initialised.
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string())?;
            }
            Ok(Self)
        }
    }
    impl Drop for MfRuntime {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }

    pub struct HwEncoder {
        bitrate: u32,
        mft: IMFTransform,
        events: Option<IMFMediaEventGenerator>,
        codec: Option<ICodecAPI>,
        width: u32,
        height: u32,
        fps: u32,
        pending_need: u32,
        provides_samples: bool,
        out_size: u32,
        seq_header: Vec<u8>,
        frame_index: i64,
        nv12: Vec<u8>,
        ready: VecDeque<Vec<u8>>,
        empty_polls: u32,
        need_wait: Duration,
        out_wait: Duration,
        gpu: Option<GpuConvert>,
        name: String,
        _runtime: MfRuntime,
    }

    fn err(context: &str, e: windows::core::Error) -> String {
        format!("{context}: {e}")
    }

    fn set_codec(codec: &Option<ICodecAPI>, key: &GUID, value: VARIANT) {
        if let Some(codec) = codec {
            unsafe {
                let _ = codec.SetValue(key, &value);
            }
        }
    }

    /// RGB→NV12 conversion on the GPU. The captured texture never leaves VRAM:
    /// the video processor writes NV12 into a render target, which is then handed
    /// to Media Foundation as a DXGI-surface-backed sample.
    #[derive(Clone)]
    pub struct GpuConvert {
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        video_device: ID3D11VideoDevice,
        video_context: ID3D11VideoContext,
        enumerator: ID3D11VideoProcessorEnumerator,
        processor: ID3D11VideoProcessor,
        /// The asynchronous MFT may still be reading a surface when the next
        /// frame arrives, so rotate through several NV12 targets.
        nv12: [Option<ID3D11Texture2D>; 3],
        output_views: [Option<ID3D11VideoProcessorOutputView>; 3],
        /// Reused for GPU→CPU NV12 readback; allocating one per frame stalls
        /// the D3D11 device at 60 fps.
        staging: Option<ID3D11Texture2D>,
        next: usize,
        size: (u32, u32),
        manager: IMFDXGIDeviceManager,
    }

    impl GpuConvert {
        pub fn new(
            device: &ID3D11Device,
            context: &ID3D11DeviceContext,
            width: u32,
            height: u32,
            fps: u32,
        ) -> Result<Self, String> {
            let video_device: ID3D11VideoDevice =
                device.cast().map_err(|e| err("video device", e))?;
            let video_context: ID3D11VideoContext =
                context.cast().map_err(|e| err("video context", e))?;
            let rate = DXGI_RATIONAL {
                Numerator: fps.max(1),
                Denominator: 1,
            };
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: width,
                InputHeight: height,
                OutputFrameRate: rate,
                OutputWidth: width,
                OutputHeight: height,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&desc) }
                .map_err(|e| err("video processor enumerator", e))?;
            let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
            unsafe { enumerator.GetVideoProcessorCaps(&mut caps) }
                .map_err(|e| err("video processor caps", e))?;
            let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
                .map_err(|e| err("video processor", e))?;
            let (mut reset_token, mut manager) = (0u32, None);
            unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut manager) }
                .map_err(|e| err("DXGI device manager", e))?;
            let manager = manager.ok_or("no DXGI device manager")?;
            unsafe { manager.ResetDevice(device, reset_token) }
                .map_err(|e| err("reset DXGI device", e))?;
            Ok(Self {
                device: device.clone(),
                context: context.clone(),
                video_device,
                video_context,
                enumerator,
                processor,
                nv12: [None, None, None],
                output_views: [None, None, None],
                staging: None,
                next: 0,
                size: (0, 0),
                manager,
            })
        }

        fn convert(
            &mut self,
            frame: &GpuFrame,
            target_width: u32,
            target_height: u32,
        ) -> Result<ID3D11Texture2D, String> {
            if self.size != (target_width, target_height) {
                let td = D3D11_TEXTURE2D_DESC {
                    Width: target_width,
                    Height: target_height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: Default::default(),
                    },
                };
                for slot in 0..self.nv12.len() {
                    let mut texture = None;
                    unsafe { self.device.CreateTexture2D(&td, None, Some(&mut texture)) }
                        .map_err(|e| err("NV12 render target", e))?;
                    let texture = texture.ok_or("no NV12 render target")?;
                    let mut view = None;
                    unsafe {
                        self.video_device.CreateVideoProcessorOutputView(
                            &texture,
                            &self.enumerator,
                            &view_desc,
                            Some(&mut view),
                        )
                    }
                    .map_err(|e| err("NV12 output view", e))?;
                    self.output_views[slot] = view;
                    self.nv12[slot] = Some(texture);
                }
                self.next = 0;
                self.size = (target_width, target_height);
                self.staging = None;
            }
            let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: Default::default(),
                },
            };
            let mut input_view = None;
            unsafe {
                self.video_device.CreateVideoProcessorInputView(
                    &frame.texture,
                    &self.enumerator,
                    &input_desc,
                    Some(&mut input_view),
                )
            }
            .map_err(|e| err("capture input view", e))?;
            let input_view = input_view.ok_or("no capture input view")?;
            let index = self.next;
            self.next = (self.next + 1) % self.nv12.len();
            let output_view = self.output_views[index]
                .clone()
                .ok_or("no NV12 output view")?;
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: windows::core::BOOL(1),
                pInputSurface: std::mem::ManuallyDrop::new(Some(input_view)),
                ..Default::default()
            };
            let blit = unsafe {
                self.video_context.VideoProcessorBlt(
                    &self.processor,
                    &output_view,
                    0,
                    std::slice::from_ref(&stream),
                )
            }
            .map_err(|e| err("RGB to NV12", e));
            // The stream struct uses ManuallyDrop for COM pointers; release the
            // per-frame input view explicitly or every frame leaks a D3D11 view.
            unsafe { std::mem::ManuallyDrop::drop(&mut stream.pInputSurface) };
            blit?;
            // Submit the conversion before the encoder reads the surface.
            unsafe { self.context.Flush() };
            self.nv12[index]
                .clone()
                .ok_or("no NV12 render target".into())
        }

        /// Convert a captured texture to packed NV12 bytes. Vulkan Video encodes
        /// from a buffer rather than a D3D11 surface, so this is the bridge
        /// between Graphics Capture and that backend.
        pub fn read_nv12(
            &mut self,
            frame: &GpuFrame,
            width: u32,
            height: u32,
        ) -> Result<Vec<u8>, String> {
            let texture = self.convert(frame, width, height)?;
            if self.staging.is_none() {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe { texture.GetDesc(&mut desc) };
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut staging = None;
                unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut staging)) }
                    .map_err(|e| err("NV12 staging texture", e))?;
                self.staging = staging;
            }
            let staging = self
                .staging
                .clone()
                .ok_or("no NV12 staging texture".to_string())?;
            unsafe { self.context.CopyResource(&staging, &texture) };
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                self.context
                    .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .map_err(|e| err("NV12 staging map", e))?;
            }
            let pitch = mapped.RowPitch as usize;
            let y_size = width as usize * height as usize;
            let mut out = vec![0u8; y_size * 3 / 2];
            unsafe {
                let src = std::slice::from_raw_parts(
                    mapped.pData as *const u8,
                    pitch * height as usize * 3 / 2,
                );
                for row in 0..height as usize {
                    let start = row * width as usize;
                    out[start..start + width as usize]
                        .copy_from_slice(&src[row * pitch..row * pitch + width as usize]);
                }
                let uv_base = pitch * height as usize;
                for row in 0..height as usize / 2 {
                    let start = y_size + row * width as usize;
                    out[start..start + width as usize].copy_from_slice(
                        &src[uv_base + row * pitch..uv_base + row * pitch + width as usize],
                    );
                }
                self.context.Unmap(&staging, 0);
            }
            Ok(out)
        }

        fn sample(&self, texture: &ID3D11Texture2D) -> Result<IMFSample, String> {
            let buffer =
                unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false) }
                    .map_err(|e| err("DXGI surface buffer", e))?;
            let sample = unsafe { MFCreateSample().map_err(|e| err("MFCreateSample", e))? };
            unsafe { sample.AddBuffer(&buffer).map_err(|e| err("AddBuffer", e))? };
            Ok(sample)
        }
    }

    impl HwEncoder {
        pub fn new(width: u32, height: u32, fps: u32, bitrate: u32) -> Result<Self, String> {
            Self::new_impl(width, height, fps, bitrate, None)
        }

        /// Hardware encoder fed by GPU textures: the picture is converted to
        /// NV12 on the GPU and the MFT reads it from a DXGI surface.
        pub fn new_with_device(
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
            device: &ID3D11Device,
            context: &ID3D11DeviceContext,
        ) -> Result<Self, String> {
            let gpu = GpuConvert::new(device, context, width, height, fps)?;
            Self::new_impl(width, height, fps, bitrate, Some(&gpu))
        }

        fn new_impl(
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
            gpu: Option<&GpuConvert>,
        ) -> Result<Self, String> {
            // Keeps Media Foundation alive while enumerating; each opened
            // encoder holds its own reference.
            let _runtime = MfRuntime::start()?;
            unsafe {
                let input = MFT_REGISTER_TYPE_INFO {
                    guidMajorType: MFMediaType_Video,
                    guidSubtype: MFVideoFormat_NV12,
                };
                let output = MFT_REGISTER_TYPE_INFO {
                    guidMajorType: MFMediaType_Video,
                    guidSubtype: MFVideoFormat_H264,
                };
                let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
                let mut count = 0u32;
                MFTEnumEx(
                    MFT_CATEGORY_VIDEO_ENCODER,
                    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
                    Some(&input),
                    Some(&output),
                    &mut list,
                    &mut count,
                )
                .map_err(|e| err("MFTEnumEx", e))?;
                let mut activates = Vec::new();
                if !list.is_null() {
                    for i in 0..count as usize {
                        if let Some(a) = (*list.add(i)).take() {
                            activates.push(a);
                        }
                    }
                    CoTaskMemFree(Some(list as *const _));
                }
                if activates.is_empty() {
                    return Err("Nenhum encoder H.264 por hardware encontrado".into());
                }
                let mut last_error = String::new();
                for activate in activates {
                    match Self::open(&activate, width, height, fps, bitrate, gpu) {
                        Ok(mut enc) => {
                            enc.name = friendly_name(&activate);
                            return Ok(enc);
                        }
                        Err(e) => last_error = e,
                    }
                }
                Err(last_error)
            }
        }

        unsafe fn open(
            activate: &IMFActivate,
            width: u32,
            height: u32,
            fps: u32,
            bitrate: u32,
            gpu: Option<&GpuConvert>,
        ) -> Result<Self, String> {
            let runtime = MfRuntime::start()?;
            let mft: IMFTransform = activate
                .ActivateObject()
                .map_err(|e| err("ActivateObject", e))?;
            let attrs = mft.GetAttributes().ok();
            let mut is_async = false;
            if let Some(attrs) = &attrs {
                is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1;
                if is_async {
                    attrs
                        .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .map_err(|e| err("ASYNC_UNLOCK", e))?;
                }
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            // The D3D manager can only be attached after the async MFT is unlocked.
            if let Some(gpu) = gpu {
                mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, gpu.manager.as_raw() as usize)
                    .map_err(|e| err("SET_D3D_MANAGER", e))?;
            }
            let events = if is_async {
                Some(
                    mft.cast::<IMFMediaEventGenerator>()
                        .map_err(|e| err("IMFMediaEventGenerator", e))?,
                )
            } else {
                None
            };
            let codec = mft.cast::<ICodecAPI>().ok();
            set_codec(&codec, &CODECAPI_AVLowLatencyMode, VARIANT::from(true));
            set_codec(
                &codec,
                &CODECAPI_AVEncCommonRateControlMode,
                // LowDelayVBR builds a delay buffer that saturates after a few
                // seconds of full motion and then throttles input to ~25 fps.
                // UnconstrainedVBR keeps accepting frames at the requested rate.
                // UnconstrainedVBR avoids the delay buffer that LowDelayVBR
                // builds up under sustained motion.
                VARIANT::from(eAVEncCommonRateControlMode_UnconstrainedVBR.0 as u32),
            );
            set_codec(
                &codec,
                &CODECAPI_AVEncCommonMeanBitRate,
                VARIANT::from(bitrate),
            );
            set_codec(
                &codec,
                &CODECAPI_AVEncMPVDefaultBPictureCount,
                VARIANT::from(0u32),
            );
            set_codec(&codec, &CODECAPI_AVEncH264CABACEnable, VARIANT::from(false));
            // Long GOP: recovery is on demand (receiver requests) plus the sender's
            // periodic IDR. Short GOPs make every IDR a big, low-quality burst.
            set_codec(
                &codec,
                &CODECAPI_AVEncMPVGOPSize,
                VARIANT::from(fps.max(1) * 10),
            );
            // Prefer quality over speed. The quantiser cap is loose: a tight one
            // (38) made frames too big for the bitrate, and the sender then had to
            // drop captures to keep to the ceiling, which halved the frame rate.
            // Vendors that do not support a key just ignore it.
            set_codec(
                &codec,
                &CODECAPI_AVEncCommonQualityVsSpeed,
                VARIANT::from(50u32),
            );
            set_codec(&codec, &CODECAPI_AVEncVideoMaxQP, VARIANT::from(46u32));

            let size = ((width as u64) << 32) | height as u64;
            let rate = ((fps.max(1) as u64) << 32) | 1;
            let aspect = (1u64 << 32) | 1;

            let out_type = MFCreateMediaType().map_err(|e| err("MFCreateMediaType", e))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| err("type", e))?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(|e| err("type", e))?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, bitrate)
                .map_err(|e| err("type", e))?;
            out_type
                .SetUINT64(&MF_MT_FRAME_SIZE, size)
                .map_err(|e| err("type", e))?;
            out_type
                .SetUINT64(&MF_MT_FRAME_RATE, rate)
                .map_err(|e| err("type", e))?;
            out_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, aspect)
                .map_err(|e| err("type", e))?;
            out_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| err("type", e))?;
            out_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base.0 as u32)
                .map_err(|e| err("type", e))?;
            mft.SetOutputType(0, &out_type, 0)
                .map_err(|e| err("SetOutputType", e))?;

            let in_type = MFCreateMediaType().map_err(|e| err("MFCreateMediaType", e))?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| err("type", e))?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| err("type", e))?;
            in_type
                .SetUINT64(&MF_MT_FRAME_SIZE, size)
                .map_err(|e| err("type", e))?;
            in_type
                .SetUINT64(&MF_MT_FRAME_RATE, rate)
                .map_err(|e| err("type", e))?;
            in_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, aspect)
                .map_err(|e| err("type", e))?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| err("type", e))?;
            mft.SetInputType(0, &in_type, 0)
                .map_err(|e| err("SetInputType", e))?;

            let info = mft
                .GetOutputStreamInfo(0)
                .map_err(|e| err("GetOutputStreamInfo", e))?;
            let provides_samples = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                    | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32)
                != 0;

            let _ = mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| err("BEGIN_STREAMING", e))?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| err("START_OF_STREAM", e))?;

            let mut enc = Self {
                bitrate,
                mft,
                events,
                codec,
                width,
                height,
                fps: fps.max(1),
                pending_need: if is_async { 0 } else { 1 },
                provides_samples,
                out_size: info.cbSize.max(1 << 20),
                seq_header: Vec::new(),
                frame_index: 0,
                nv12: Vec::new(),
                ready: VecDeque::new(),
                empty_polls: 0,
                need_wait: Duration::ZERO,
                out_wait: Duration::ZERO,
                gpu: gpu.cloned(),
                name: String::new(),
                _runtime: runtime,
            };
            enc.refresh_sequence_header();
            Ok(enc)
        }

        /// Waiting times for the last frames, in milliseconds: NeedInput and output.
        pub fn take_waits(&mut self) -> (f32, f32) {
            (
                std::mem::take(&mut self.need_wait).as_secs_f32() * 1000.0,
                std::mem::take(&mut self.out_wait).as_secs_f32() * 1000.0,
            )
        }

        pub fn size(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), String> {
            if bitrate == self.bitrate {
                return Ok(());
            }
            let codec = self
                .codec
                .as_ref()
                .ok_or("Encoder de GPU não permite ajustar bitrate")?;
            unsafe {
                codec
                    .SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bitrate))
                    .map_err(|e| err("ajustar bitrate da GPU", e))?;
            }
            self.bitrate = bitrate;
            Ok(())
        }

        fn refresh_sequence_header(&mut self) {
            unsafe {
                if let Ok(current) = self.mft.GetOutputCurrentType(0) {
                    if let Ok(len) = current.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                        let mut blob = vec![0u8; len as usize];
                        if current
                            .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)
                            .is_ok()
                        {
                            self.seq_header = blob;
                        }
                    }
                }
            }
        }

        /// Returns the next finished access unit, or `None` when the encoder
        /// did not produce one yet. The bool is true for IDR frames.
        pub fn encode(
            &mut self,
            pixels: &[u8],
            format: PixelFormat,
            force: bool,
        ) -> Result<Option<(Vec<u8>, bool)>, String> {
            let (w, h) = (self.width as usize, self.height as usize);
            if pixels.len() != w * h * 4 {
                return Err("Tamanho de quadro inesperado para o encoder de GPU".into());
            }
            pixels_to_nv12(pixels, w, h, &mut self.nv12, format);
            let sample = unsafe { self.make_sample()? };
            self.process_sample(sample, force)
        }

        /// Encode a captured texture without copying the picture to CPU memory.
        pub fn encode_texture(
            &mut self,
            frame: &GpuFrame,
            force: bool,
        ) -> Result<Option<(Vec<u8>, bool)>, String> {
            if (frame.width, frame.height) != (self.width, self.height) {
                return Err("Tamanho de quadro inesperado para o encoder de GPU".into());
            }
            let convert = self.gpu.as_mut().ok_or("Conversão de GPU indisponível")?;
            let texture = convert.convert(frame, self.width, self.height)?;
            let sample = convert.sample(&texture)?;
            self.process_sample(sample, force)
        }

        fn process_sample(
            &mut self,
            sample: IMFSample,
            force: bool,
        ) -> Result<Option<(Vec<u8>, bool)>, String> {
            unsafe {
                if force && self.frame_index > 0 {
                    set_codec(
                        &self.codec,
                        &CODECAPI_AVEncVideoForceKeyFrame,
                        VARIANT::from(1u32),
                    );
                }
                let duration = 10_000_000i64 / self.fps as i64;
                sample
                    .SetSampleTime(self.frame_index * duration)
                    .map_err(|e| err("SetSampleTime", e))?;
                sample
                    .SetSampleDuration(duration)
                    .map_err(|e| err("SetSampleDuration", e))?;
                // An async MFT announces when it wants input.
                let deadline = Instant::now() + Duration::from_millis(500);
                let need_started = Instant::now();
                while self.pending_need == 0 {
                    self.pump(deadline)?;
                }
                self.need_wait += need_started.elapsed();
                self.mft
                    .ProcessInput(0, &sample, 0)
                    .map_err(|e| err("ProcessInput", e))?;
                self.pending_need = self.pending_need.saturating_sub(1);
                self.frame_index += 1;
                if self.events.is_none() {
                    self.drain_sync()?;
                    self.pending_need = 1;
                } else {
                    let deadline = Instant::now() + Duration::from_millis(250);
                    let out_started = Instant::now();
                    while self.ready.is_empty() && Instant::now() < deadline {
                        self.pump(deadline)?;
                    }
                    self.out_wait += out_started.elapsed();
                }
            }
            let Some(mut data) = self.ready.pop_front() else {
                self.empty_polls += 1;
                if self.empty_polls > 8 {
                    return Err("Encoder de GPU parou de produzir quadros".into());
                }
                return Ok(None);
            };
            self.empty_polls = 0;
            let keyframe = has_nal(&data, 5);
            if keyframe && !has_nal(&data, 7) {
                self.refresh_sequence_header();
                if self.seq_header.is_empty() {
                    return Err("IDR sem SPS/PPS do encoder de GPU".into());
                }
                let mut with_header = self.seq_header.clone();
                with_header.extend_from_slice(&data);
                data = with_header;
            }
            Ok(Some((data, keyframe)))
        }

        unsafe fn make_sample(&self) -> Result<IMFSample, String> {
            let buffer = MFCreateMemoryBuffer(self.nv12.len() as u32)
                .map_err(|e| err("MFCreateMemoryBuffer", e))?;
            let mut ptr = std::ptr::null_mut();
            buffer
                .Lock(&mut ptr, None, None)
                .map_err(|e| err("Lock", e))?;
            std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), ptr, self.nv12.len());
            buffer.Unlock().map_err(|e| err("Unlock", e))?;
            buffer
                .SetCurrentLength(self.nv12.len() as u32)
                .map_err(|e| err("SetCurrentLength", e))?;
            let sample = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
            sample.AddBuffer(&buffer).map_err(|e| err("AddBuffer", e))?;
            let duration = 10_000_000i64 / self.fps as i64;
            sample
                .SetSampleTime(self.frame_index * duration)
                .map_err(|e| err("SetSampleTime", e))?;
            sample
                .SetSampleDuration(duration)
                .map_err(|e| err("SetSampleDuration", e))?;
            Ok(sample)
        }

        /// Handles at most one pending event from an async MFT.
        unsafe fn pump(&mut self, deadline: Instant) -> Result<(), String> {
            let Some(events) = self.events.clone() else {
                return Ok(());
            };
            match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(event) => {
                    let kind = event.GetType().map_err(|e| err("GetType", e))?;
                    if kind == METransformNeedInput.0 as u32 {
                        self.pending_need += 1;
                    } else if kind == METransformHaveOutput.0 as u32 {
                        if let Some(data) = self.take_output()? {
                            self.ready.push_back(data);
                        }
                    } else if kind == MEError.0 as u32 {
                        let status = event.GetStatus().map(|h| h.0).unwrap_or(-1);
                        return Err(format!("Evento de erro do encoder de GPU: {status:#x}"));
                    }
                    Ok(())
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Err("Tempo esgotado esperando o encoder de GPU".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    Ok(())
                }
                Err(e) => Err(err("GetEvent", e)),
            }
        }

        unsafe fn drain_sync(&mut self) -> Result<(), String> {
            while let Some(data) = self.take_output()? {
                self.ready.push_back(data);
            }
            Ok(())
        }

        unsafe fn take_output(&mut self) -> Result<Option<Vec<u8>>, String> {
            let provided = if self.provides_samples {
                None
            } else {
                let buffer = MFCreateMemoryBuffer(self.out_size)
                    .map_err(|e| err("MFCreateMemoryBuffer", e))?;
                let sample = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
                sample.AddBuffer(&buffer).map_err(|e| err("AddBuffer", e))?;
                Some(sample)
            };
            let mut out = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(provided),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let result = self.mft.ProcessOutput(0, &mut out, &mut status);
            let sample = ManuallyDrop::take(&mut out[0].pSample);
            let _events = ManuallyDrop::take(&mut out[0].pEvents);
            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    return Err("Mudança de formato inesperada no encoder de GPU".into())
                }
                Err(e) => return Err(err("ProcessOutput", e)),
            }
            let Some(sample) = sample else {
                return Ok(None);
            };
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| err("ConvertToContiguousBuffer", e))?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(|e| err("Lock", e))?;
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            let _ = buffer.Unlock();
            Ok((!data.is_empty()).then_some(data))
        }
    }

    impl Drop for HwEncoder {
        fn drop(&mut self) {
            unsafe {
                let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
                let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            }
        }
    }

    fn friendly_name(activate: &IMFActivate) -> String {
        unsafe {
            let mut buffer = [0u16; 128];
            let mut len = 0u32;
            if activate
                .GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buffer, Some(&mut len))
                .is_ok()
            {
                return String::from_utf16_lossy(&buffer[..len as usize]);
            }
        }
        "GPU".into()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn nv12_gray_is_neutral_chroma() {
            let rgba = vec![128u8; 8 * 4 * 4];
            let mut out = Vec::new();
            pixels_to_nv12(&rgba, 8, 4, &mut out, PixelFormat::Rgba);
            assert_eq!(out.len(), 8 * 4 * 3 / 2);
            assert!(out[..32].iter().all(|v| (125..=127).contains(v)));
            assert!(out[32..].iter().all(|v| (127..=129).contains(v)));
        }

        #[test]
        #[ignore = "manual 1080p RGB to NV12 conversion benchmark"]
        fn benchmark_pixels_to_nv12() {
            let (w, h) = (1920usize, 1080usize);
            let pixels = vec![128u8; w * h * 4];
            let mut out = Vec::new();
            let mut timings = Vec::new();
            for _ in 0..60 {
                let start = std::time::Instant::now();
                pixels_to_nv12(&pixels, w, h, &mut out, PixelFormat::Bgra);
                timings.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            timings.sort_by(f64::total_cmp);
            eprintln!(
                "NV12 1080p: mean {:.2} ms, p50 {:.2} ms, p95 {:.2} ms",
                timings.iter().sum::<f64>() / timings.len() as f64,
                timings[30],
                timings[57]
            );
        }
    }
}
