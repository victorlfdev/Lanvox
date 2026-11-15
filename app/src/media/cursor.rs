//! The mouse pointer of the machine sharing its screen. Screen capture (GDI and desktop
//! duplication alike) leaves the pointer out, so it is sampled here and sent apart from
//! the video: the viewer draws it at its own refresh rate, and moving it costs no encode.

#[cfg(not(windows))]
use crate::protocol::CursorShape;

/// A screen area in desktop pixels.
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Where the visible pointer is on the desktop and which pointer image it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sample {
    pub x: i32,
    pub y: i32,
    pub id: u32,
}

#[cfg(windows)]
pub use imp::{client_rect, sample, shape};

#[cfg(not(windows))]
pub fn sample() -> Option<Sample> {
    None
}
#[cfg(not(windows))]
pub fn client_rect(_window: u32) -> Option<Rect> {
    None
}
#[cfg(not(windows))]
pub fn shape() -> Option<CursorShape> {
    None
}

/// The pointer hotspot as a fraction of `area` (0..=65535), or `None` when it is
/// outside the area (a window share shows no pointer beyond the window).
pub fn to_fraction(sample: Sample, area: Rect) -> Option<(u16, u16)> {
    if area.w == 0 || area.h == 0 {
        return None;
    }
    let fx = f64::from(sample.x - area.x) / f64::from(area.w);
    let fy = f64::from(sample.y - area.y) / f64::from(area.h);
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return None;
    }
    Some(((fx * 65535.0).round() as u16, (fy * 65535.0).round() as u16))
}

/// Combine a pointer drawn on black and on white into straight RGBA. Alpha follows from
/// how much the white background shows through; inverting pointers (no alpha, XOR mask)
/// come out opaque in the colour they take over black.
#[cfg_attr(not(windows), allow(dead_code))]
fn compose(on_black: &[u8], on_white: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(on_black.len());
    for (b, w) in on_black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(on_white.as_chunks::<4>().0.iter())
    {
        // Both are BGRA from a DIB.
        let diff: i32 = (0..3)
            .map(|c| i32::from(w[c]) - i32::from(b[c]))
            .sum::<i32>()
            / 3;
        let alpha = 255 - diff;
        if alpha <= 0 {
            // White background turned darker than black: an inverting pixel.
            let lum = (i32::from(b[0]) + i32::from(b[1]) + i32::from(b[2])) / 3;
            if lum > 96 {
                out.extend_from_slice(&[b[2], b[1], b[0], 255]);
            } else {
                out.extend_from_slice(&[0, 0, 0, 0]);
            }
            continue;
        }
        let alpha = alpha.min(255);
        let unpremultiply = |c: u8| ((i32::from(c) * 255 + alpha / 2) / alpha).clamp(0, 255) as u8;
        out.extend_from_slice(&[
            unpremultiply(b[2]),
            unpremultiply(b[1]),
            unpremultiply(b[0]),
            alpha as u8,
        ]);
    }
    out
}

#[cfg(windows)]
mod imp {
    use super::{compose, Rect, Sample};
    use crate::protocol::{CursorShape, MAX_CURSOR_SIDE};
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetObjectW,
        SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DrawIconEx, GetCursorInfo, GetIconInfo, GetWindowInfo, CURSORINFO, CURSOR_SHOWING,
        DI_NORMAL, ICONINFO, WINDOWINFO,
    };

    /// The visible pointer's handle and screen position.
    fn current() -> Option<(*mut c_void, i32, i32)> {
        unsafe {
            let mut info: CURSORINFO = zeroed();
            info.cbSize = size_of::<CURSORINFO>() as u32;
            if GetCursorInfo(&mut info) == 0
                || info.flags & CURSOR_SHOWING == 0
                || info.hCursor.is_null()
            {
                return None;
            }
            Some((info.hCursor, info.ptScreenPos.x, info.ptScreenPos.y))
        }
    }

    /// Where the pointer is on the desktop and which image it has, when it is visible.
    pub fn sample() -> Option<Sample> {
        current().map(|(cursor, x, y)| Sample {
            x,
            y,
            id: cursor as usize as u32,
        })
    }

    /// The client area of a window (`xcap` captures this area).
    pub fn client_rect(window: u32) -> Option<Rect> {
        unsafe {
            let mut info: WINDOWINFO = zeroed();
            info.cbSize = size_of::<WINDOWINFO>() as u32;
            if GetWindowInfo(window as usize as *mut c_void, &mut info) == 0 {
                return None;
            }
            let r = info.rcClient;
            Some(Rect {
                x: r.left,
                y: r.top,
                w: (r.right - r.left).max(0) as u32,
                h: (r.bottom - r.top).max(0) as u32,
            })
        }
    }

    /// Renders the current pointer image to RGBA with transparency.
    pub fn shape() -> Option<CursorShape> {
        let (cursor, _, _) = current()?;
        unsafe {
            let mut icon: ICONINFO = zeroed();
            if GetIconInfo(cursor, &mut icon) == 0 {
                return None;
            }
            // A pointer without a colour bitmap keeps AND and XOR masks stacked in one.
            let monochrome = icon.hbmColor.is_null();
            let mut bitmap: BITMAP = zeroed();
            let measured = GetObjectW(
                if monochrome {
                    icon.hbmMask
                } else {
                    icon.hbmColor
                },
                size_of::<BITMAP>() as i32,
                &mut bitmap as *mut BITMAP as *mut c_void,
            );
            let (hot_x, hot_y) = (icon.xHotspot, icon.yHotspot);
            if !icon.hbmMask.is_null() {
                DeleteObject(icon.hbmMask);
            }
            if !icon.hbmColor.is_null() {
                DeleteObject(icon.hbmColor);
            }
            if measured == 0 {
                return None;
            }
            let native_w = bitmap.bmWidth.max(1) as u32;
            let native_h = (if monochrome {
                bitmap.bmHeight / 2
            } else {
                bitmap.bmHeight
            })
            .max(1) as u32;
            let side = u32::from(MAX_CURSOR_SIDE);
            let scale = (f64::from(side) / f64::from(native_w.max(native_h))).min(1.0);
            let w = ((f64::from(native_w) * scale).round() as u32).clamp(1, side);
            let h = ((f64::from(native_h) * scale).round() as u32).clamp(1, side);

            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bmi: BITMAPINFO = zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            bmi.bmiHeader.biHeight = -(h as i32); // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = null_mut();
            let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let len = w as usize * h as usize * 4;
            let surface = std::slice::from_raw_parts_mut(bits as *mut u8, len);
            let old = SelectObject(dc, dib);
            let mut passes = [Vec::new(), Vec::new()];
            let mut drawn = true;
            for (pass, fill) in passes.iter_mut().zip([0u8, 255u8]) {
                surface.fill(fill);
                drawn &= DrawIconEx(
                    dc,
                    0,
                    0,
                    cursor,
                    w as i32,
                    h as i32,
                    0,
                    null_mut(),
                    DI_NORMAL,
                ) != 0;
                GdiFlush();
                *pass = surface.to_vec();
            }
            SelectObject(dc, old);
            DeleteObject(dib);
            DeleteDC(dc);
            if !drawn {
                return None;
            }
            let rgba = compose(&passes[0], &passes[1]);
            if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
                return None;
            }
            Some(CursorShape {
                id: cursor as usize as u32,
                w: w as u16,
                h: h as u16,
                hot_x: ((f64::from(hot_x) * scale).round() as u32).min(w - 1) as u16,
                hot_y: ((f64::from(hot_y) * scale).round() as u32).min(h - 1) as u16,
                rgba,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_is_relative_to_the_shared_area_and_hidden_outside_it() {
        let area = Rect {
            x: 100,
            y: 50,
            w: 200,
            h: 100,
        };
        let at = |x, y| Sample { x, y, id: 1 };
        assert_eq!(to_fraction(at(100, 50), area), Some((0, 0)));
        assert_eq!(to_fraction(at(300, 150), area), Some((65535, 65535)));
        let mid = to_fraction(at(200, 100), area).unwrap();
        assert!((i32::from(mid.0) - 32768).abs() <= 1 && (i32::from(mid.1) - 32768).abs() <= 1);
        assert_eq!(to_fraction(at(99, 100), area), None);
        assert_eq!(to_fraction(at(200, 151), area), None);
    }

    #[test]
    fn compose_recovers_alpha_and_colour() {
        // BGRA: half-transparent red = premultiplied (0,0,128) on black, +127 on white.
        let on_black = [0, 0, 128, 0];
        let on_white = [127, 127, 255, 255];
        let rgba = compose(&on_black, &on_white);
        assert_eq!(rgba[3], 128);
        assert!(rgba[0] >= 254 && rgba[1] == 0 && rgba[2] == 0);
        // Fully opaque white stays white; untouched background stays clear.
        assert_eq!(
            compose(&[255, 255, 255, 0], &[255, 255, 255, 0]),
            [255, 255, 255, 255]
        );
        assert_eq!(compose(&[0, 0, 0, 0], &[255, 255, 255, 255])[3], 0);
    }

    #[cfg(windows)]
    #[test]
    fn the_real_pointer_renders_to_a_valid_visible_shape() {
        // Headless sessions may have no visible pointer; when there is one it must be sane.
        if let Some(shape) = shape() {
            assert!(shape.valid());
            assert!(shape.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        }
    }
}
