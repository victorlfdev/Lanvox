//! Windows Graphics Capture: the capture path that sees what games draw.
//!
//! GDI (`PrintWindow`/`BitBlt`) cannot read DirectX swap chains, so a game window
//! comes out black, and desktop duplication drops out for good the first time a
//! fullscreen game changes the display mode. Graphics Capture reads the
//! compositor's own copy of a window or a screen, survives mode switches and is
//! what other streaming apps use for games.

use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows::core::{factory, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use super::screen::SourceId;

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const BUFFERS: i32 = 2;

fn err(context: &str, e: windows::core::Error) -> String {
    format!("{context}: {e}")
}

/// Handles are listed as `u32`; Windows keeps them sign-extended 32-bit values.
fn handle(id: u32) -> *mut core::ffi::c_void {
    id as i32 as isize as *mut core::ffi::c_void
}

pub struct WgcCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    arrived: Receiver<()>,
    size: SizeInt32,
    /// Two staging textures: the copy of the new frame is queued into one while the
    /// previous frame is read from the other, so the CPU never waits for the GPU
    /// to finish a copy queued behind a game's own rendering.
    staging: [Option<ID3D11Texture2D>; 2],
    staging_size: (u32, u32),
    /// Index of the texture holding the copy of the previous frame, if any.
    pending: Option<(usize, u32, u32)>,
    next_staging: usize,
    window: Option<HWND>,
}

impl WgcCapture {
    pub fn supported() -> bool {
        GraphicsCaptureSession::IsSupported().unwrap_or(false)
    }

    pub fn new(id: SourceId) -> Result<Self, String> {
        // WinRT needs the thread in a COM apartment; "already initialized" is fine.
        let _ = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
        };
        if !Self::supported() {
            return Err("Graphics Capture unsupported".into());
        }
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .map_err(|e| err("capture interop", e))?;
        let (item, window): (GraphicsCaptureItem, Option<HWND>) = unsafe {
            match id {
                SourceId::Monitor(m) => (
                    interop
                        .CreateForMonitor(HMONITOR(handle(m)))
                        .map_err(|e| err("capture screen", e))?,
                    None,
                ),
                SourceId::Window(w) => {
                    let hwnd = HWND(handle(w));
                    (
                        interop
                            .CreateForWindow(hwnd)
                            .map_err(|e| err("capture window", e))?,
                        Some(hwnd),
                    )
                }
            }
        };

        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| err("D3D11 device", e))?;
        }
        let device = device.ok_or("no D3D11 device")?;
        let context = context.ok_or("no D3D11 context")?;
        let dxgi: IDXGIDevice = device.cast().map_err(|e| err("DXGI device", e))?;
        let winrt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .and_then(|i| i.cast())
            .map_err(|e| err("WinRT device", e))?;

        let size = item.Size().map_err(|e| err("capture size", e))?;
        let pool =
            Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, FORMAT, BUFFERS, size)
                .map_err(|e| err("frame pool", e))?;
        let (tx, arrived) = sync_channel(1);
        pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
            let _ = tx.try_send(());
            Ok(())
        }))
        .map_err(|e| err("frame event", e))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| err("capture session", e))?;
        // The pointer is sent apart from the video, and the yellow border is
        // only a hint (Windows 11); both are optional.
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|e| err("start capture", e))?;

        Ok(Self {
            device,
            context,
            winrt_device,
            pool,
            session,
            arrived,
            size,
            staging: [None, None],
            staging_size: (0, 0),
            pending: None,
            next_staging: 0,
            window,
        })
    }

    /// The newest picture, waiting up to `wait` for one. `Ok(None)` means nothing
    /// changed (or a window is minimized); `Err` means the source is gone.
    pub fn grab(&mut self, wait: Duration) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        if let Some(hwnd) = self.window {
            if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                return Err("window was closed".into());
            }
        }
        let deadline = Instant::now() + wait;
        loop {
            let mut newest: Option<Direct3D11CaptureFrame> = None;
            while let Ok(frame) = self.pool.TryGetNextFrame() {
                if let Some(old) = newest.replace(frame) {
                    let _ = old.Close();
                }
            }
            if let Some(frame) = newest {
                let result = self.read(&frame);
                let _ = frame.Close();
                return result;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return self.flush_pending();
            }
            match self.arrived.recv_timeout(left) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => return self.flush_pending(),
                Err(RecvTimeoutError::Disconnected) => return Err("capture stopped".into()),
            }
        }
    }

    /// Maps one staging texture and converts it to RGBA.
    fn read_staging(
        &mut self,
        index: usize,
        pw: u32,
        ph: u32,
    ) -> Result<(u32, u32, Vec<u8>), String> {
        let staging = self.staging[index].clone().ok_or("no staging texture")?;

        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| err("map frame", e))?;
            let pitch = mapped.RowPitch as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * ph as usize);
            let row = pw as usize * 4;
            let mut rgba = vec![0u8; row * ph as usize];
            // BGRA -> RGBA, rows spread over the rayon pool.
            use rayon::prelude::*;
            rgba.par_chunks_exact_mut(row)
                .enumerate()
                .for_each(|(y, out)| {
                    let line = &src[y * pitch..y * pitch + row];
                    for (o, i) in out
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(line.as_chunks::<4>().0)
                    {
                        *o = [i[2], i[1], i[0], 255];
                    }
                });
            self.context.Unmap(&staging, 0);
            Ok((pw, ph, rgba))
        }
    }

    /// The copy queued by the last frame, when no newer frame came to push it out
    /// (a still screen would otherwise keep showing an older picture).
    fn flush_pending(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        match self.pending.take() {
            Some((index, w, h)) => self.read_staging(index, w, h).map(Some),
            None => Ok(None),
        }
    }

    fn read(
        &mut self,
        frame: &Direct3D11CaptureFrame,
    ) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        let content = frame.ContentSize().map_err(|e| err("frame size", e))?;
        if content.Width <= 0 || content.Height <= 0 {
            return Ok(None);
        }
        if content.Width != self.size.Width || content.Height != self.size.Height {
            // The window was resized or the game changed resolution: new buffers,
            // and this frame is skipped since it was drawn for the old size.
            self.size = content;
            self.pool
                .Recreate(&self.winrt_device, FORMAT, BUFFERS, content)
                .map_err(|e| err("resize frame pool", e))?;
            return Ok(None);
        }
        let surface = frame.Surface().map_err(|e| err("frame surface", e))?;
        let access: IDirect3DDxgiInterfaceAccess =
            surface.cast().map_err(|e| err("surface access", e))?;
        let texture: ID3D11Texture2D =
            unsafe { access.GetInterface() }.map_err(|e| err("frame texture", e))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let w = (content.Width as u32).min(desc.Width);
        let h = (content.Height as u32).min(desc.Height);

        if self.staging_size != (desc.Width, desc.Height) || self.staging[0].is_none() {
            let mut sd = desc;
            sd.Usage = D3D11_USAGE_STAGING;
            sd.BindFlags = 0;
            sd.MiscFlags = 0;
            sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            sd.MipLevels = 1;
            sd.ArraySize = 1;
            for slot in &mut self.staging {
                let mut t = None;
                unsafe { self.device.CreateTexture2D(&sd, None, Some(&mut t)) }
                    .map_err(|e| err("staging texture", e))?;
                *slot = Some(t.ok_or("no staging texture")?);
            }
            self.staging_size = (desc.Width, desc.Height);
            self.pending = None;
        }
        // Queue the copy of this frame, then read the one queued last time.
        let write = self.next_staging;
        self.next_staging ^= 1;
        let target = self.staging[write].clone().ok_or("no staging texture")?;
        unsafe { self.context.CopyResource(&target, &texture) };
        let previous = self.pending.replace((write, w, h));
        let Some((read, pw, ph)) = previous else {
            // First frame: nothing to show yet; the next grab returns this one.
            return Ok(None);
        };
        self.read_staging(read, pw, ph).map(Some)
    }
}

impl Drop for WgcCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs a desktop session"]
    fn captures_primary_screen() {
        let monitor = xcap::Monitor::all()
            .unwrap()
            .into_iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .unwrap();
        let mut cap = WgcCapture::new(SourceId::Monitor(monitor.id().unwrap())).unwrap();
        let mut got = None;
        for _ in 0..20 {
            if let Some(p) = cap.grab(Duration::from_millis(200)).unwrap() {
                got = Some(p);
                break;
            }
        }
        let (w, h, rgba) = got.expect("no frame");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] | p[1] | p[2] != 0),
            "all black"
        );
    }
}
