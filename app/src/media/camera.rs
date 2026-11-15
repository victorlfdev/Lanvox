//! Camera video for calls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::codec::{encode_jpeg, scale_rgba};
use super::video::PictureStore;
use crate::model::{StreamKind, UserId};
use crate::net::client::ClientSender;
use crate::protocol::{Tile, VideoFrame};

/// Camera picture size and rate.
pub const CAMERA_MAX_W: u32 = 1280;
pub const CAMERA_MAX_H: u32 = 720;
pub const CAMERA_FPS: u32 = 30;
const CAMERA_QUALITY: u8 = 70;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CameraResolution {
    Sd,
    #[default]
    Hd,
    FullHd,
}

impl CameraResolution {
    pub const ALL: [Self; 3] = [Self::Sd, Self::Hd, Self::FullHd];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sd => "480p",
            Self::Hd => "720p",
            Self::FullHd => "1080p",
        }
    }
    pub fn size(self) -> (u32, u32) {
        match self {
            Self::Sd => (640, 480),
            Self::Hd => (CAMERA_MAX_W, CAMERA_MAX_H),
            Self::FullHd => (1920, 1080),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    pub resolution: CameraResolution,
    pub fps: u32,
    /// The local preview can be mirrored; transmitted frames always retain their orientation.
    pub mirror: bool,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            resolution: CameraResolution::Hd,
            fps: CAMERA_FPS,
            mirror: true,
        }
    }
}

impl CameraSettings {
    pub fn normalized(self) -> Self {
        Self {
            fps: if self.fps <= 15 { 15 } else { 30 },
            ..self
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CameraStats {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub encoder: String,
    pub adaptation: u32,
}

fn camera_bitrate(width: u32, height: u32) -> u32 {
    (width.saturating_mul(height).saturating_mul(3)).clamp(700_000, 6_000_000)
}

/// Packed RGB to RGBA with opaque alpha, in one pass over a preallocated
/// buffer so the loop vectorises.
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut rgba = vec![255u8; rgb.len() / 3 * 4];
    for (dst, src) in rgba
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(rgb.as_chunks::<3>().0.iter())
    {
        dst[..3].copy_from_slice(src);
    }
    rgba
}

pub fn camera_names() -> Vec<String> {
    nokhwa::query(nokhwa::utils::ApiBackend::Auto)
        .map(|v| v.into_iter().map(|c| c.human_name()).collect())
        .unwrap_or_default()
}

/// Open camera `index`, trying several format requests because drivers differ in what they offer.
#[cfg(test)]
pub fn open_camera(index: u32) -> Result<nokhwa::Camera, String> {
    open_camera_configured(index, CameraSettings::default())
}

pub fn open_camera_configured(
    index: u32,
    settings: CameraSettings,
) -> Result<nokhwa::Camera, String> {
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    };
    let mut last = String::from("no usable video format");
    let formats = [
        FrameFormat::MJPEG,
        FrameFormat::YUYV,
        FrameFormat::NV12,
        FrameFormat::RAWRGB,
        FrameFormat::GRAY,
    ];
    let mut requests: Vec<RequestedFormat> = Vec::new();
    let settings = settings.normalized();
    let (wanted_w, wanted_h) = settings.resolution.size();
    for (fi, ff) in formats.iter().copied().enumerate() {
        for (w, h, fps) in [
            (wanted_w, wanted_h, settings.fps),
            (640, 480, 30),
            (1280, 720, 30),
            (640, 480, 15),
            (320, 240, 30),
        ] {
            requests.push(RequestedFormat::with_formats(
                RequestedFormatType::Exact(CameraFormat::new(Resolution::new(w, h), ff, fps)),
                &formats[fi..fi + 1],
            ));
        }
    }
    requests.push(RequestedFormat::new::<RgbFormat>(RequestedFormatType::None));
    requests.push(RequestedFormat::new::<RgbFormat>(
        RequestedFormatType::AbsoluteHighestFrameRate,
    ));
    requests.push(RequestedFormat::new::<RgbFormat>(
        RequestedFormatType::AbsoluteHighestResolution,
    ));
    requests.push(RequestedFormat::new::<RgbFormat>(
        RequestedFormatType::Closest(CameraFormat::new(
            Resolution::new(640, 480),
            FrameFormat::MJPEG,
            30,
        )),
    ));
    for req in requests {
        match nokhwa::Camera::new(CameraIndex::Index(index), req) {
            Ok(mut cam) => match cam.open_stream() {
                Ok(()) => return Ok(cam),
                Err(e) => last = e.to_string(),
            },
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}
pub struct CameraSession {
    stop: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    pub stats: Arc<Mutex<CameraStats>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for CameraSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wait for the device to be released so a new session can open the same/another camera.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl CameraSession {
    /// Starts exactly the chosen camera. Settings bound output quality without upscaling.
    pub fn start_configured(
        index: u32,
        sender: Option<ClientSender>,
        preview: PictureStore,
        me: UserId,
        ctx: egui::Context,
        settings: CameraSettings,
    ) -> CameraSession {
        let settings = settings.normalized();
        let (max_w, max_h) = settings.resolution.size();
        let target_fps = settings.fps;
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(CameraStats::default()));
        let thread = {
            let stop = stop.clone();
            let error = error.clone();
            let stats = stats.clone();
            std::thread::Builder::new()
                .name("camera".into())
                .spawn(move || {
                    use nokhwa::pixel_format::RgbFormat;
                    // Open exactly the chosen camera; never silently fall back to another one.
                    let name = camera_names()
                        .get(index as usize)
                        .cloned()
                        .unwrap_or_default();
                    let mut cam = match open_camera_configured(index, settings) {
                        Ok(c) => c,
                        Err(e) => {
                            *error.lock() =
                                Some(format!("Não foi possível abrir a câmera \"{name}\": {e}"));
                            ctx.request_repaint();
                            return;
                        }
                    };
                    let mut bad_frames = 0u32;
                    let mut tick = Duration::from_secs_f64(1.0 / target_fps as f64);
                    let mut seq = 0u32;
                    let mut motion = super::temporal::TemporalEncoder::new(
                        camera_bitrate(max_w, max_h),
                        target_fps,
                        false,
                    )
                    .ok();
                    let mut force = true;
                    let mut last_key = Instant::now();
                    let mut adaptive_level = 0u32;
                    let mut adaptation = Instant::now();
                    let mut dropped = false;
                    let mut clean_windows = 0;
                    let mut stats_started = Instant::now();
                    let mut stats_frames = 0u32;
                    while !stop.load(Ordering::SeqCst) {
                        let started = Instant::now();
                        let img = cam
                            .frame()
                            .map_err(|e| e.to_string())
                            .and_then(|b| b.decode_image::<RgbFormat>().map_err(|e| e.to_string()));
                        let img = match img {
                            Ok(i) => {
                                bad_frames = 0;
                                i
                            }
                            Err(e) => {
                                bad_frames += 1;
                                if bad_frames > 100 {
                                    *error.lock() =
                                        Some(format!("A câmera parou de enviar imagens: {e}"));
                                    ctx.request_repaint();
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(50));
                                continue;
                            }
                        };
                        let (sw, sh) = (img.width(), img.height());
                        let rgba = rgb_to_rgba(&img.into_raw());
                        let s = (max_w as f32 / sw as f32)
                            .min(max_h as f32 / sh as f32)
                            .min(1.0)
                            * 0.75f32.powi(adaptive_level as i32);
                        let w = ((sw as f32 * s) as u32).max(2) & !1;
                        let h = ((sh as f32 * s) as u32).max(2) & !1;
                        // At native size there is nothing to scale: reuse the buffer.
                        let pic = if (w, h) == (sw, sh) {
                            rgba
                        } else {
                            scale_rgba(&rgba, sw, sh, w, h)
                        };
                        if let Some(sender) = &sender {
                            if sender.is_closed() {
                                break;
                            }
                            seq = seq.wrapping_add(1);
                            if let Some(encoder) = &mut motion {
                                if force
                                    || sender.take_camera_keyframe()
                                    || last_key.elapsed() > Duration::from_secs(2)
                                {
                                    force = true;
                                    last_key = Instant::now();
                                }
                                match encoder.encode(&pic, w, h, force) {
                                    Ok(Some(frame)) => {
                                        force =
                                            sender.send_encoded_frame(StreamKind::Camera, frame);
                                        dropped |= force;
                                    }
                                    Ok(None) => {}
                                    Err(_) => {
                                        motion = None;
                                        force = true;
                                    }
                                }
                            } else {
                                let jpeg = encode_jpeg(&pic, w, h, CAMERA_QUALITY);
                                sender.send_frame(
                                    StreamKind::Camera,
                                    VideoFrame {
                                        seq,
                                        width: w,
                                        height: h,
                                        keyframe: true,
                                        tiles: vec![Tile {
                                            x: 0,
                                            y: 0,
                                            w: w as u16,
                                            h: h as u16,
                                            jpeg,
                                        }],
                                    },
                                );
                            }
                            if adaptation.elapsed() >= Duration::from_secs(1) {
                                let previous_level = adaptive_level;
                                if dropped || sender.media_stats().queued_bytes > 150_000 {
                                    adaptive_level = (adaptive_level + 1).min(2);
                                    clean_windows = 0;
                                } else {
                                    clean_windows += 1;
                                    if clean_windows >= 5 && adaptive_level > 0 {
                                        adaptive_level -= 1;
                                        clean_windows = 0;
                                    }
                                }
                                if adaptive_level != previous_level {
                                    let fps = target_fps / (1 << adaptive_level);
                                    tick = Duration::from_secs_f64(1.0 / f64::from(fps));
                                    motion = super::temporal::TemporalEncoder::new(
                                        camera_bitrate(max_w, max_h) / (1 << adaptive_level),
                                        fps,
                                        false,
                                    )
                                    .ok();
                                    force = true;
                                }
                                dropped = false;
                                adaptation = Instant::now();
                            }
                        }
                        stats_frames += 1;
                        if stats_started.elapsed() >= Duration::from_secs(1) {
                            *stats.lock() = CameraStats {
                                width: w,
                                height: h,
                                fps: stats_frames as f32 / stats_started.elapsed().as_secs_f32(),
                                encoder: motion
                                    .as_ref()
                                    .map(|e| e.backend())
                                    .unwrap_or_else(|| "JPEG".into()),
                                adaptation: adaptive_level,
                            };
                            stats_started = Instant::now();
                            stats_frames = 0;
                        }
                        // Mirroring belongs to presentation, never the shared source.
                        let mut pic = pic;
                        if settings.mirror {
                            for row in pic.chunks_exact_mut(w as usize * 4) {
                                let pixels = row.as_chunks_mut::<4>().0;
                                pixels.reverse();
                            }
                        }
                        preview.put((me, StreamKind::Camera), w, h, pic, 0);
                        ctx.request_repaint();
                        let spent = started.elapsed();
                        if spent < tick {
                            std::thread::sleep(tick - spent);
                        }
                    }
                    let _ = cam.stop_stream();
                })
                .ok()
        };
        CameraSession {
            stop,
            error,
            stats,
            thread,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires an available camera and reads real camera frames"]
    fn camera_probe() {
        let names = super::camera_names();
        let mut received_frame = false;
        eprintln!("cameras: {names:?}");
        for idx in 0..names.len() as u32 {
            match super::open_camera(idx) {
                Err(e) => eprintln!("camera {idx}: {e}"),
                Ok(mut cam) => {
                    eprintln!("camera {idx}: opened {:?}", cam.camera_format());
                    for i in 0..30 {
                        match cam.frame() {
                            Ok(b) => {
                                use nokhwa::pixel_format::RgbFormat;
                                match b.decode_image::<RgbFormat>() {
                                    Ok(img) => {
                                        assert!(img.width() > 0 && img.height() > 0);
                                        received_frame = true;
                                        eprintln!(
                                            "camera {idx}: frame {i} {}x{}",
                                            img.width(),
                                            img.height()
                                        );
                                        break;
                                    }
                                    Err(e) => eprintln!("camera {idx}: decode: {e}"),
                                }
                            }
                            Err(e) => eprintln!("camera {idx}: frame {i}: {e}"),
                        }
                    }
                    let _ = cam.stop_stream();
                }
            }
        }
        assert!(received_frame, "no camera produced a decodable frame");
    }
}

#[cfg(test)]
mod rgba_tests {
    #[test]
    fn rgb_to_rgba_adds_opaque_alpha() {
        assert_eq!(
            super::rgb_to_rgba(&[1, 2, 3, 4, 5, 6]),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
    }
}
