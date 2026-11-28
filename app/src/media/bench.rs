//! Manual benchmarks for the whole screen-share pipeline. Ignored by default:
//! they capture the real desktop and open loopback sockets.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect, HBRUSH, HDC,
    PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW,
    SetProcessDPIAware, ShowWindow, TranslateMessage, UnregisterClassW, MSG, PM_REMOVE, SW_SHOW,
    WM_PAINT, WNDCLASSW,
};

use super::screen::{list_sources, ScreenShare, ShareSource, SourceId, StreamSettings};
use super::temporal::TemporalEncoder;
use super::video::PictureStore;
use super::wgc::WgcCapture;
use crate::net::client::ClientSender;
use crate::net::client::KeyframeFlags;
use crate::net::outbox::Outbox;

fn filetime_seconds(ft: FILETIME) -> f64 {
    ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) as f64 / 10_000_000.0
}

/// Process-wide user + kernel CPU seconds, for the whole test binary.
fn cpu_seconds() -> (f64, f64) {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return (0.0, 0.0);
        }
    }
    (filetime_seconds(user), filetime_seconds(kernel))
}

static PAINT_COLOR: AtomicU32 = AtomicU32::new(0x00203040);

unsafe extern "system" fn bench_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_PAINT {
        let mut paint = PAINTSTRUCT::default();
        let hdc: HDC = BeginPaint(hwnd, &mut paint);
        let brush: HBRUSH = CreateSolidBrush(PAINT_COLOR.load(Ordering::Relaxed));
        FillRect(hdc, &paint.rcPaint, brush);
        DeleteObject(brush);
        EndPaint(hwnd, &paint);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// A real 1920x1080 window whose painted content changes every frame, so
/// Graphics Capture delivers full-resolution frames instead of a still screen.
struct MovingWindow {
    hwnd: HWND,
    class: Vec<u16>,
    module: HWND,
}

impl MovingWindow {
    fn source_id(&self) -> SourceId {
        SourceId::Window(self.hwnd as usize as u32)
    }

    /// Repaint a different colour and pump the queue on the owning thread.
    fn animate(&self, position: i32) {
        PAINT_COLOR.store(
            0x00200000 | (position as u32 % 0xd0) << 8,
            Ordering::Relaxed,
        );
        unsafe {
            InvalidateRect(self.hwnd, std::ptr::null(), 1);
            // Dispatch below paints the window. UpdateWindow would block this
            // thread on paint completion and miss the 60 Hz source cadence.
            let mut message = MSG::default();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl MovingWindow {
    fn start() -> Option<Self> {
        // Without DPI awareness Windows scales the window, so a 1920x1080
        // request would be captured as a smaller logical picture.
        unsafe { SetProcessDPIAware() };
        let class: Vec<u16> = "LanvoxBenchWindow\0".encode_utf16().collect();
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(bench_wnd_proc),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&wc) } == 0 {
            return None;
        }
        let title: Vec<u16> = "Lanvox benchmark\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0x90000000, // WS_POPUP | WS_VISIBLE: client area is exactly 1920x1080
                0,
                0,
                1920,
                1080,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                module,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            unsafe { UnregisterClassW(class.as_ptr(), module) };
            return None;
        }
        unsafe { ShowWindow(hwnd, SW_SHOW) };
        Some(Self {
            hwnd,
            class,
            module,
        })
    }
}

impl Drop for MovingWindow {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            UnregisterClassW(self.class.as_ptr(), self.module);
        }
    }
}

fn loopback_sender() -> (ClientSender, Arc<AtomicU32>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("local address");
    let writer = thread::Builder::new()
        .spawn(move || TcpStream::connect(address).expect("connect"))
        .expect("connect thread");
    let (client, _) = listener.accept().expect("accept");
    let reader = writer.join().expect("connect result");
    let received = Arc::new(AtomicU32::new(0));
    let counter = received.clone();
    thread::Builder::new()
        .name("bench-drain".into())
        .spawn(move || {
            let mut buffer = [0u8; 64 * 1024];
            let mut stream = reader;
            while let Ok(n) = stream.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                counter.fetch_add(n as u32, Ordering::Relaxed);
            }
        })
        .expect("drain thread");
    (ClientSender::new(Outbox::start(client)), received)
}

#[test]
#[ignore = "manual GPU encoder wait breakdown with live Graphics Capture"]
fn benchmark_gpu_encoder_waits_with_capture() {
    let source = list_sources(false)
        .into_iter()
        .find(|s| matches!(s.id, SourceId::Monitor(_)))
        .expect("an accessible display is required");
    let window = MovingWindow::start().expect("animated window");
    let mut capture = WgcCapture::new(source.id).expect("Graphics Capture");
    let mut encoder = TemporalEncoder::new(12_000_000, 60, true).expect("H.264 encoder");
    let (mut need_total, mut out_total, mut encode_total) = (0.0f32, 0.0f32, 0.0f32);
    let (mut window_need, mut window_out, mut window_encode, mut window_frames) =
        (0.0f32, 0.0f32, 0.0f32, 0u32);
    let mut frames = 0u32;
    for i in 0..600u32 {
        window.animate((i * 4) as i32);
        let grabbed = capture.grab(Duration::from_millis(16)).expect("grab");
        if let Some((w, h, pixels)) = grabbed {
            let started = Instant::now();
            let _ = encoder.encode_bgra(&pixels, w, h, i == 0).expect("encode");
            let encode_ms = started.elapsed().as_secs_f32() * 1000.0;
            let (need, out) = encoder.take_hw_waits();
            need_total += need;
            out_total += out;
            encode_total += encode_ms;
            window_need += need;
            window_out += out;
            window_encode += encode_ms;
            window_frames += 1;
            frames += 1;
            if i % 60 == 0 && window_frames > 0 {
                // A fresh window of phase timings each second exposes a growing
                // wait in the asynchronous Media Foundation encoder.
                eprintln!(
                    "frame {i}: {w}x{h} encode {:.2} ms | NeedInput {:.2} ms | output {:.2} ms",
                    window_encode / window_frames as f32,
                    window_need / window_frames as f32,
                    window_out / window_frames as f32
                );
                (window_need, window_out, window_encode, window_frames) = (0.0, 0.0, 0.0, 0);
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    let frames_f = frames.max(1) as f32;
    eprintln!(
        "GPU waits with live capture: {} backend, {frames} frames, encode {:.2} ms | NeedInput {:.2} ms | output {:.2} ms",
        encoder.backend(),
        encode_total / frames_f,
        need_total / frames_f,
        out_total / frames_f
    );
}

#[test]
#[ignore = "manual end-to-end 1080p60 screen-share benchmark; captures the desktop"]
fn benchmark_1080p60_screen_share_e2e() {
    // A real 1920x1080 window with changing content: the exact target size,
    // rather than fitting a 2260x1080 monitor into a 1080p pixel budget.
    let window = MovingWindow::start().expect("animated 1920x1080 window");
    let source = ShareSource {
        id: window.source_id(),
        name: "1920x1080 benchmark".into(),
        app: String::new(),
        width: 1920,
        height: 1080,
        thumb: None,
        process_id: None,
    };
    let settings = StreamSettings {
        resolution: super::screen::Resolution::P1080,
        fps: 60,
        // Acceptance test: hold 1920x1080 and measure the encoder, rather than
        // letting adaptation hide a stall by lowering the resolution.
        adaptive: false,
        ..StreamSettings::default()
    };
    let (sender, received) = loopback_sender();
    let keyframes = Arc::new(KeyframeFlags::default());
    keyframes.screen_viewers.store(1, Ordering::Relaxed);
    let (user_before, kernel_before) = cpu_seconds();
    let started = Instant::now();
    let share = ScreenShare::start(
        &source,
        settings,
        sender,
        keyframes,
        PictureStore::default(),
        1,
        egui::Context::default(),
    );
    let mut samples = Vec::new();
    let window_ref = &window;
    for second in 1..=6 {
        for tick in 0..60 {
            // Drive the source at exactly 60 Hz. A fixed 16 ms sleep runs at
            // 62.5 Hz wall time and makes WGC coalesce frames, under-reporting
            // the pipeline's achievable cadence.
            let deadline =
                started + Duration::from_millis(((second - 1) * 1000 + tick * 1000 / 60) as u64);
            window_ref.animate(tick * 4 + second * 17);
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
        }
        let deadline = started + Duration::from_secs(second as u64);
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        let s = share.stats.lock().clone();
        samples.push((
            second,
            s.width,
            s.height,
            s.codec.clone(),
            s.fps,
            s.capture_fps,
            s.capture_events,
            s.encoded_fps,
            s.offered_fps,
            s.handled_fps,
            s.encode_ms,
            s.encode_avg_ms,
            s.work_ms,
            s.kbps,
        ));
    }
    let wall = started.elapsed();
    let (user_after, kernel_after) = cpu_seconds();
    let bytes = received.load(Ordering::Relaxed);
    drop(share);
    drop(window);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let cpu = ((user_after - user_before) + (kernel_after - kernel_before)) / wall.as_secs_f64();
    for (
        second,
        w,
        h,
        codec,
        fps,
        cap,
        events,
        enc,
        offered,
        handled,
        encode_ms,
        encode_avg,
        work,
        kbps,
    ) in &samples
    {
        eprintln!(
            "s{second}: {w}x{h} out {fps:.1} fps | capture {cap:.1} fps ({events} events) | offered {offered:.1} handled {handled:.1} encoded {enc:.1} | encode {encode_ms:.2}/{encode_avg:.2} ms, work {work:.2} ms | {kbps:.0} kbit/s | {codec}"
        );
    }
    eprintln!(
        "E2E: {:.1} s wall, {} bytes sent, process CPU {:.0}% of one core ({:.0}% of {} cores)",
        wall.as_secs_f64(),
        bytes,
        cpu * 100.0,
        cpu / cores * 100.0,
        cores as u32
    );
    // Manual benchmark: report the numbers rather than gating on a particular
    // GPU vendor or on a busy CI machine.
    assert!(samples.iter().any(|s| s.4 > 0.0), "no frames were streamed");
}

/// Probe the Vulkan Video backend. It uses a completely different driver path
/// from Media Foundation, so it can bypass the AMD MFT input stall. Reports
/// whether this adapter exposes H.264 encode and whether it sustains 1080p60.
#[test]
#[ignore = "manual GPU benchmark"]
fn benchmark_vulkan_video_encoder() {
    use std::num::NonZeroU32;

    use gpu_video::parameters::{
        EncoderParametersH264, RateControl, VideoDeviceDescriptor, VideoParameters,
    };
    use gpu_video::{InputFrame, RawFrameRef, VideoAdapterExt, VideoDeviceExt};

    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const FRAMES: u32 = 600;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::default()));
    eprintln!("VULKAN: {} wgpu adapters", adapters.len());
    let mut chosen = None;
    for adapter in adapters {
        let info = adapter.video_adapter_info();
        match &info {
            Some(info) => eprintln!(
                "VULKAN adapter {:?}: H.264 encode {:?}",
                adapter.get_info().name,
                info.encode_capabilities.h264
            ),
            None => eprintln!(
                "VULKAN adapter {:?}: no video info",
                adapter.get_info().name
            ),
        }
        if chosen.is_none() && info.is_some_and(|i| i.encode_capabilities.h264.is_some()) {
            chosen = Some(adapter);
        }
    }
    let Some(adapter) = chosen else {
        eprintln!("VULKAN: no adapter with H.264 encode support");
        return;
    };

    let (device, _queue) = adapter
        .request_device_with_video_support(&VideoDeviceDescriptor::default())
        .expect("video device");
    let video_device = device.video().expect("video device extension");
    let params = EncoderParametersH264 {
        output_parameters: video_device
            .encoder_output_parameters_h264_low_latency(RateControl::VariableBitrate {
                average_bitrate: 60_000_000,
                max_bitrate: 72_000_000,
                virtual_buffer_size: Duration::from_millis(500),
            })
            .expect("output parameters"),
        input_parameters: VideoParameters {
            width: NonZeroU32::new(WIDTH as u32).unwrap(),
            height: NonZeroU32::new(HEIGHT as u32).unwrap(),
            target_framerate: 60.into(),
        },
        max_in_flight_submissions: None,
    };
    let bytes_out = Arc::new(AtomicU32::new(0));
    let keyframes = Arc::new(AtomicU32::new(0));
    let chunk_bytes = Arc::clone(&bytes_out);
    let chunk_keys = Arc::clone(&keyframes);
    let mut encoder = video_device
        .create_bytes_encoder_h264(params, move |chunk| {
            chunk_bytes.fetch_add(chunk.data.len() as u32, Ordering::Relaxed);
            if chunk.is_keyframe {
                chunk_keys.fetch_add(1, Ordering::Relaxed);
            }
        })
        .expect("Vulkan H.264 encoder");

    let y_size = WIDTH * HEIGHT;
    let mut frame = vec![0u8; y_size * 3 / 2];
    let started = Instant::now();
    let mut last = started;
    let mut in_second = 0u32;
    let mut second = 0;
    let (cpu_user_before, cpu_kernel_before) = cpu_seconds();
    for i in 0..FRAMES {
        // Changing full-frame content keeps the encoder under continuous load.
        for (n, pixel) in frame[..y_size].iter_mut().enumerate() {
            *pixel = ((n + i as usize * 37) & 0xff) as u8;
        }
        let frame_start = Instant::now();
        encoder
            .encode(
                &InputFrame {
                    data: RawFrameRef {
                        frame: &frame,
                        width: WIDTH as u32,
                        height: HEIGHT as u32,
                    },
                    pts: Some(i as u64),
                },
                i % 60 == 0,
            )
            .expect("encode frame");
        in_second += 1;
        if last.elapsed() >= Duration::from_secs(1) {
            second += 1;
            eprintln!(
                "VULKAN s{second}: {in_second} fps | encode {:.2} ms | {} kbit/s",
                frame_start.elapsed().as_secs_f32() * 1000.0,
                bytes_out.swap(0, Ordering::Relaxed) as f64 * 8.0 / 1000.0,
            );
            in_second = 0;
            last = Instant::now();
        }
    }
    encoder.flush().expect("flush encoder");
    let wall = started.elapsed();
    let (cpu_user_after, cpu_kernel_after) = cpu_seconds();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let cpu = ((cpu_user_after - cpu_user_before) + (cpu_kernel_after - cpu_kernel_before))
        / wall.as_secs_f64();
    eprintln!(
        "VULKAN RESULT: {:.1} fps average, {} keyframes, CPU {:.0}% of one core ({:.0}% of {} cores)",
        FRAMES as f64 / wall.as_secs_f64(),
        keyframes.load(Ordering::Relaxed),
        cpu * 100.0,
        cpu / cores * 100.0,
        cores as u32
    );
}
