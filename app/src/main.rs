//! Lanvox: voice, video, text and screen sharing over the local network.
//!
//! Layout of the code:
//! - `model`, `protocol`: what the app talks about and how it is sent.
//! - `net`: the hosted server, the client connection, finding servers.
//! - `media`: voice, camera, screen capture and the picture pipeline.
//! - `state`: the client's copy of the server.
//! - `app`: the controller tying it together; `ui`: everything drawn.

#![cfg_attr(
    all(feature = "gui", not(debug_assertions)),
    windows_subsystem = "windows"
)]
#![cfg_attr(not(feature = "gui"), allow(dead_code))]

#[cfg(feature = "gui")]
mod app;
mod auth;
mod branding;
mod categories;
#[cfg(feature = "gui")]
mod demo;
#[cfg(feature = "gui")]
mod file_transfer;
mod host_backup;
mod host_cli;
mod invites;
mod media;
#[cfg(feature = "gui")]
mod media_cache;
mod mentions;
mod model;
mod net;
#[cfg(feature = "gui")]
mod notifications;
mod protocol;
mod secret_store;
#[cfg(feature = "gui")]
mod settings;
#[cfg(feature = "gui")]
mod sfx;
#[cfg(feature = "gui")]
mod startup;
#[cfg(feature = "gui")]
mod state;
mod storage;
#[cfg(feature = "gui")]
mod ui;
mod updater;

#[cfg(feature = "gui")]
use std::path::PathBuf;

fn run_host(args: &[String]) {
    #[cfg(all(feature = "gui", windows))]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
    if let Err(error) = host_cli::run(args) {
        eprintln!("Lanvox: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "gui"))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    branding::migrate_legacy_folders();
    run_host(&args);
}

#[cfg(feature = "gui")]
fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    branding::migrate_legacy_folders();
    if host_cli::requested(&args) {
        run_host(&args);
        return Ok(());
    }
    if let Some(invite) = args.iter().find(|arg| branding::is_link(arg)) {
        if notifications::forward_invite(invite) {
            return Ok(());
        }
    }
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    #[cfg(windows)]
    let _instance = if screenshot.is_none() {
        match startup::DesktopInstance::acquire() {
            Ok(Some(instance)) => Some(instance),
            Ok(None) => {
                // The owning process may still be creating its tray window.
                for _ in 0..40 {
                    let delivered =
                        if let Some(invite) = args.iter().find(|arg| branding::is_link(arg)) {
                            notifications::forward_invite(invite)
                        } else {
                            notifications::forward_open()
                        };
                    if delivered {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                return Ok(());
            }
            Err(error) => {
                eprintln!("Não foi possível verificar a instância do Lanvox: {error}");
                return Ok(());
            }
        }
    } else {
        None
    };

    let screenshot_size = args
        .iter()
        .position(|arg| arg == "--screenshot-size")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.split_once('x'))
        .and_then(|(width, height)| Some([width.parse::<f32>().ok()?, height.parse::<f32>().ok()?]))
        .filter(|size| {
            size[0].is_finite()
                && size[1].is_finite()
                && size[0] >= 940.0
                && size[1] >= 500.0
                && size[0] <= 3840.0
                && size[1] <= 2160.0
        })
        .unwrap_or([1280.0, 720.0]);

    // Default window size and minimum size.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Lanvox")
        .with_inner_size(screenshot_size)
        .with_min_inner_size([940.0, 500.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    if screenshot.is_some() {
        // Don't steal focus from the person while a check runs.
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    // Keep drawing even when the window is not focused or is covered.
    options.run_and_return = true;
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            builder.with_activate_ignoring_other_apps(false);
        }));
    }

    let result = eframe::run_native(
        "Lanvox",
        options,
        Box::new(move |cc| Ok(Box::new(app::LanvoxApp::new(cc, screenshot)))),
    );
    notifications::shutdown();
    result
}
