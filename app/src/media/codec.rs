//! Picture handling for streams: fitting a resolution under a pixel budget,
//! scaling, and the tile encoder that only sends what changed.

use rayon::prelude::*;

use crate::protocol::{Tile, VideoFrame};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PixelFormat {
    #[default]
    Rgba,
    Bgra,
}

/// Produces the RGBA layout expected by egui's image upload path.
pub fn into_rgba(pixels: &[u8], format: PixelFormat) -> Vec<u8> {
    if format == PixelFormat::Rgba {
        return pixels.to_vec();
    }
    let mut rgba = vec![0; pixels.len()];
    rgba.par_chunks_exact_mut(4)
        .zip(pixels.par_chunks_exact(4))
        .for_each(|(out, input)| out.copy_from_slice(&[input[2], input[1], input[0], 255]));
    rgba
}

/// Side of one square tile in pixels.
pub const TILE: u32 = 64;
/// Lowest tile quality the bitrate control goes down to (estimated).
pub const MIN_QUALITY: u8 = 40;
/// Remainders (mod 16) a fitted width/height may have.
const ALIGNED_RESIDUES: [u32; 4] = [0, 4, 8, 10];

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Largest size with the source's aspect ratio whose pixel count fits `budget`,
/// computed as follows:
/// reduce W:H to a:b, then take the largest k with (k·a)·(k·b) ≤ budget where both
/// sides have an accepted remainder mod 16; otherwise scale by √(budget / (W·H)).
/// A budget of 0 (or one at least the source size) keeps the source size.
pub fn fit_resolution(width: u32, height: u32, budget: u64) -> (u32, u32) {
    let pixels = width as u64 * height as u64;
    if budget == 0 || pixels <= budget || width == 0 || height == 0 {
        return (width.max(2) & !1, height.max(2) & !1);
    }
    let g = gcd(width, height);
    let (a, b) = (width / g, height / g);
    let k_max = ((budget as f64 * a as f64 / b as f64).sqrt() / a as f64) as u32;
    let mut k = k_max;
    while k >= 1 {
        let (w, h) = (k * a, k * b);
        if (w as u64) * (h as u64) <= budget
            && ALIGNED_RESIDUES.contains(&(w % 16))
            && ALIGNED_RESIDUES.contains(&(h % 16))
        {
            return (w.min(width), h.min(height));
        }
        k -= 1;
    }
    let s = (budget as f64 / pixels as f64).sqrt();
    let w = ((width as f64 * s) as u32).max(2) & !1;
    let h = ((height as f64 * s) as u32).max(2) & !1;
    (w, h)
}

/// Bilinear scale of an RGBA picture.
pub fn scale_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    let x_ratio = sw as f32 / dw as f32;
    let y_ratio = sh as f32 / dh as f32;
    // Precompute horizontal sample positions.
    let xs: Vec<(usize, usize, f32)> = (0..dw)
        .map(|x| {
            let fx = ((x as f32 + 0.5) * x_ratio - 0.5).max(0.0);
            let x0 = (fx as u32).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            (x0 as usize * 4, x1 as usize * 4, fx - x0 as f32)
        })
        .collect();
    out.par_chunks_mut((dw * 4) as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let fy = ((y as f32 + 0.5) * y_ratio - 0.5).max(0.0);
            let y0 = (fy as u32).min(sh - 1);
            let y1 = (y0 + 1).min(sh - 1);
            let ty = fy - y0 as f32;
            let r0 = &src[(y0 * sw * 4) as usize..((y0 + 1) * sw * 4) as usize];
            let r1 = &src[(y1 * sw * 4) as usize..((y1 + 1) * sw * 4) as usize];
            for (x, &(a, b, tx)) in xs.iter().enumerate() {
                for c in 0..4 {
                    let top = r0[a + c] as f32 * (1.0 - tx) + r0[b + c] as f32 * tx;
                    let bot = r1[a + c] as f32 * (1.0 - tx) + r1[b + c] as f32 * tx;
                    row[x * 4 + c] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
                }
            }
        });
    out
}

pub fn encode_jpeg(rgba: &[u8], w: u32, h: u32, quality: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h / 4) as usize);
    let enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    let _ = enc.encode(rgba, w as u16, h as u16, jpeg_encoder::ColorType::Rgba);
    out
}

pub fn decode_jpeg(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    crate::protocol::picture_bytes(width, height)?;
    let reader =
        || image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    if reader().into_dimensions().ok()? != (width, height) {
        return None;
    }
    let mut reader = reader();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let rgba = img.to_rgba8();
    Some(rgba.into_raw())
}

/// Keeps the previous picture and sends only tiles that changed.
pub struct TileEncoder {
    prev: Vec<u8>,
    width: u32,
    height: u32,
    seq: u32,
}

impl Default for TileEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl TileEncoder {
    pub fn new() -> Self {
        Self {
            prev: Vec::new(),
            width: 0,
            height: 0,
            seq: 0,
        }
    }

    /// Encode a picture. Returns None when nothing changed and no full picture is due.
    pub fn encode(
        &mut self,
        rgba: &[u8],
        w: u32,
        h: u32,
        quality: u8,
        force_key: bool,
    ) -> Option<VideoFrame> {
        let key = force_key || w != self.width || h != self.height || self.prev.len() != rgba.len();
        let cols = w.div_ceil(TILE);
        let rows = h.div_ceil(TILE);
        let stride = (w * 4) as usize;
        let prev = &self.prev;
        let tiles: Vec<Tile> = (0..rows * cols)
            .into_par_iter()
            .filter_map(|i| {
                let tx = (i % cols) * TILE;
                let ty = (i / cols) * TILE;
                let tw = TILE.min(w - tx);
                let th = TILE.min(h - ty);
                let changed = key
                    || (ty..ty + th).any(|y| {
                        let s = y as usize * stride + tx as usize * 4;
                        let e = s + tw as usize * 4;
                        rgba[s..e] != prev[s..e]
                    });
                if !changed {
                    return None;
                }
                let mut buf = Vec::with_capacity((tw * th * 4) as usize);
                for y in ty..ty + th {
                    let s = y as usize * stride + tx as usize * 4;
                    buf.extend_from_slice(&rgba[s..s + tw as usize * 4]);
                }
                Some(Tile {
                    x: tx as u16,
                    y: ty as u16,
                    w: tw as u16,
                    h: th as u16,
                    jpeg: encode_jpeg(&buf, tw, th, quality),
                })
            })
            .collect();
        self.prev.clear();
        self.prev.extend_from_slice(rgba);
        self.width = w;
        self.height = h;
        if tiles.is_empty() {
            return None;
        }
        self.seq = self.seq.wrapping_add(1);
        Some(VideoFrame {
            seq: self.seq,
            width: w,
            height: h,
            keyframe: key,
            tiles,
        })
    }
}

/// Adjusts tile quality so the stream stays near its target bitrate.
pub struct RateControl {
    pub quality: u8,
    pub max_quality: u8,
    pub target_bps: u64,
    bytes: u64,
    window: std::time::Instant,
    pub last_bps: u64,
}

impl RateControl {
    pub fn new(max_quality: u8, target_bps: u64) -> Self {
        Self {
            quality: max_quality.min(85),
            max_quality,
            target_bps,
            bytes: 0,
            window: std::time::Instant::now(),
            last_bps: 0,
        }
    }

    pub fn record(&mut self, bytes: usize) {
        self.bytes += bytes as u64;
        let el = self.window.elapsed().as_secs_f64();
        if el >= 1.0 {
            let bps = (self.bytes as f64 * 8.0 / el) as u64;
            self.last_bps = bps;
            if bps > self.target_bps {
                self.quality = self.quality.saturating_sub(5).max(MIN_QUALITY);
            } else if (bps as f64) < self.target_bps as f64 * 0.7 {
                self.quality = (self.quality + 5).min(self.max_quality);
            }
            self.bytes = 0;
            self.window = std::time::Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_1080p_into_720p_budget() {
        assert_eq!(fit_resolution(1920, 1080, 1280 * 720), (1280, 720));
    }

    #[test]
    fn keeps_small_sources() {
        assert_eq!(fit_resolution(800, 600, 1920 * 1080), (800, 600));
    }

    #[test]
    fn odd_aspect_uses_fallback_or_aligned() {
        let (w, h) = fit_resolution(1366, 768, 1280 * 720);
        assert!((w as u64) * (h as u64) <= 1280 * 720);
    }
}
