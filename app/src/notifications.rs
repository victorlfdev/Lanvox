//! Desktop integration. Private keys are never included in notifications or logs.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

#[derive(Clone, Debug)]
pub enum DesktopAction {
    Open,
    OpenMessage {
        host: String,
        channel: u32,
        message: u64,
    },
    OpenInvite(String),
    ToggleMute,
    ToggleDeafen,
    Settings,
    Exit,
}

#[cfg(windows)]
mod keyboard;

pub fn start(ctx: egui::Context) -> Receiver<DesktopAction> {
    let (tx, rx) = mpsc::channel();
    #[cfg(windows)]
    native::start(tx, ctx);
    #[cfg(not(windows))]
    drop((tx, ctx));
    rx
}

/// Registers system-wide shortcuts (work while another app, such as a game, has focus).
/// Returns nothing; use `hotkey_status` to learn whether each one was accepted.
pub fn set_hotkeys(mute: crate::settings::Hotkey, deafen: crate::settings::Hotkey) {
    #[cfg(windows)]
    {
        keyboard::record(false);
        native::set_hotkeys(mute, deafen);
    }
    #[cfg(not(windows))]
    let _ = (mute, deafen);
}

/// (mute registered, deafen registered) as of the last registration attempt.
pub fn hotkey_status() -> (bool, bool) {
    #[cfg(windows)]
    return native::hotkey_status();
    #[cfg(not(windows))]
    (false, false)
}

/// Native capture preserves the side of standalone modifier keys.
#[cfg(windows)]
pub fn record_hotkey(active: bool) {
    keyboard::record(active);
}

#[cfg(windows)]
pub fn take_recorded_hotkey() -> Option<crate::settings::Hotkey> {
    keyboard::take_recorded()
}

pub fn notify(title: &str, body: &str) {
    #[cfg(windows)]
    native::notify(title, body);
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .args(["--app-name=Lanvox", "--", title, body])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let script = "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run";
        let _ = std::process::Command::new("osascript")
            .args(["-e", script, "--", title, body])
            .spawn();
    }
}

pub fn notify_message(title: &str, body: &str, host: &str, channel: u32, message: u64) {
    #[cfg(windows)]
    native::notify_target(title, body, Some((host.to_owned(), channel, message)));
    #[cfg(not(windows))]
    {
        let _ = (host, channel, message);
        notify(title, body);
    }
}

/// Delivers a clicked invitation to the existing application without connecting automatically.
pub fn forward_invite(invite: &str) -> bool {
    #[cfg(windows)]
    return native::forward_invite(invite);
    #[cfg(not(windows))]
    {
        let _ = invite;
        false
    }
}

pub fn shutdown() {
    #[cfg(windows)]
    native::shutdown();
}

#[cfg(windows)]
pub fn forward_open() -> bool {
    native::forward_open()
}

pub fn network_addresses() -> Vec<(String, std::net::IpAddr)> {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .map(|interface| (interface.name.clone(), interface.ip()))
        .filter(|(_, ip)| !ip.is_unspecified())
        .collect();
    addresses.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    addresses.dedup();
    addresses
}

fn identity_path() -> io::Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join("Lanvox").join("identity.key"))
        .ok_or_else(|| io::Error::other("A pasta de configuração não está disponível."))
}

fn validate_identity(bytes: &[u8]) -> io::Result<ring::signature::Ed25519KeyPair> {
    ring::signature::Ed25519KeyPair::from_pkcs8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Arquivo de identidade inválido.",
        )
    })
}

fn write_private_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Exports only to a new destination; accidental replacement is rejected.
pub fn export_identity(destination: &Path) -> io::Result<()> {
    let bytes = crate::secret_store::read(&identity_path()?)?;
    validate_identity(&bytes)?;
    write_private_new(destination, &bytes)
}

/// The UI must disconnect before replacing the active identity.
pub fn import_identity(source: &Path) -> io::Result<()> {
    let bytes = crate::secret_store::read(source)?;
    validate_identity(&bytes)?;
    replace_identity(&identity_path()?, &bytes)
}

fn replace_identity(destination: &Path, bytes: &[u8]) -> io::Result<()> {
    validate_identity(bytes)?;
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("Destino de identidade inválido."))?;
    std::fs::create_dir_all(parent)?;
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let temp = parent.join(format!("identity-import-{suffix}.tmp"));
    crate::secret_store::write_new(&temp, bytes)?;
    let result = (|| {
        match std::fs::read(destination) {
            Ok(previous) => {
                // Keep a recoverable copy even if the prior file is corrupt.
                let backup = parent.join(format!("identity-backup-{suffix}.key"));
                match crate::secret_store::read(destination) {
                    Ok(decoded) => crate::secret_store::write_new(&backup, &decoded)?,
                    Err(_) => write_private_new(&backup, &previous)?,
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        std::fs::rename(&temp, destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn identity_fingerprint() -> io::Result<String> {
    use ring::signature::KeyPair;
    let key = validate_identity(&crate::secret_store::read(&identity_path()?)?)?;
    Ok(key
        .public_key()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(windows)]
mod native {
    use super::DesktopAction;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::{mpsc::Sender, Mutex, OnceLock};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static ACTIONS: OnceLock<Sender<DesktopAction>> = OnceLock::new();
    type MessageTarget = (String, u32, u64);
    type PendingNotification = (String, String, Option<MessageTarget>);
    static PENDING: Mutex<VecDeque<PendingNotification>> = Mutex::new(VecDeque::new());
    static ACTIVE: Mutex<Option<(std::time::Instant, Option<MessageTarget>)>> = Mutex::new(None);
    static WINDOW: AtomicIsize = AtomicIsize::new(0);
    const CALLBACK: u32 = WM_APP + 41;
    const OPEN: u32 = WM_APP + 43;
    const INVITE_COPY_DATA: usize = 0x48445534;

    pub(super) fn forward_open() -> bool {
        unsafe {
            let window = FindWindowW(wide("LanvoxNotificationWindow").as_ptr(), std::ptr::null());
            !window.is_null() && PostMessageW(window, OPEN, 0, 0) != 0
        }
    }

    pub(super) fn forward_invite(invite: &str) -> bool {
        if crate::invites::parse(invite).is_err() {
            return false;
        }
        let value = wide(invite);
        unsafe {
            let window = FindWindowW(wide("LanvoxNotificationWindow").as_ptr(), std::ptr::null());
            if window.is_null() {
                return false;
            }
            let data = COPYDATASTRUCT {
                dwData: INVITE_COPY_DATA,
                cbData: (value.len() * 2) as u32,
                lpData: value.as_ptr().cast::<std::ffi::c_void>() as *mut _,
            };
            let mut result = 0usize;
            SendMessageTimeoutW(
                window,
                WM_COPYDATA,
                0,
                &data as *const _ as isize,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            ) != 0
                && result == 1
        }
    }
    pub(super) fn hotkey_status() -> (bool, bool) {
        super::keyboard::status()
    }

    pub(super) fn set_hotkeys(mute: crate::settings::Hotkey, deafen: crate::settings::Hotkey) {
        super::keyboard::set_hotkeys([mute, deafen]);
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn copy_text<const N: usize>(target: &mut [u16; N], value: &str) {
        target.fill(0);
        let mut used = 0;
        for character in value.chars().filter(|c| *c != '\0') {
            let mut encoded = [0; 2];
            let units = character.encode_utf16(&mut encoded);
            if used + units.len() >= N {
                break;
            }
            target[used..used + units.len()].copy_from_slice(units);
            used += units.len();
        }
    }

    fn icon_data(window: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            ..unsafe { std::mem::zeroed() }
        }
    }

    static REPAINT: OnceLock<egui::Context> = OnceLock::new();

    pub(super) fn send(action: DesktopAction) {
        if let Some(tx) = ACTIONS.get() {
            let _ = tx.send(action);
            if let Some(ctx) = REPAINT.get() {
                ctx.request_repaint();
            }
        }
    }

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            OPEN => {
                send(DesktopAction::Open);
                0
            }
            WM_COPYDATA => {
                if lparam == 0 {
                    return 0;
                }
                let data = unsafe { &*(lparam as *const COPYDATASTRUCT) };
                if data.dwData != INVITE_COPY_DATA
                    || data.lpData.is_null()
                    || data.cbData < 2
                    || data.cbData > 32_768
                    || !data.cbData.is_multiple_of(2)
                {
                    return 0;
                }
                let units = unsafe {
                    std::slice::from_raw_parts(data.lpData.cast::<u16>(), data.cbData as usize / 2)
                };
                let units = units.strip_suffix(&[0]).unwrap_or(units);
                let Ok(invite) = String::from_utf16(units) else {
                    return 0;
                };
                if crate::invites::parse(&invite).is_err() {
                    return 0;
                }
                send(DesktopAction::OpenInvite(invite));
                1
            }
            CALLBACK => {
                match lparam as u32 {
                    NIN_BALLOONUSERCLICK => {
                        let target = ACTIVE
                            .lock()
                            .ok()
                            .and_then(|mut active| active.take())
                            .and_then(|(_, target)| target);
                        if let Some((host, channel, message)) = target {
                            send(DesktopAction::OpenMessage {
                                host,
                                channel,
                                message,
                            });
                        } else {
                            send(DesktopAction::Open);
                        }
                    }
                    NIN_BALLOONHIDE | NIN_BALLOONTIMEOUT => {
                        if let Ok(mut active) = ACTIVE.lock() {
                            *active = None;
                        }
                    }
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(DesktopAction::Open),
                    WM_RBUTTONUP => unsafe {
                        let menu = CreatePopupMenu();
                        if !menu.is_null() {
                            for (id, text) in [
                                (1, "Abrir Lanvox"),
                                (2, "Ativar/desativar microfone"),
                                (3, "Ativar/desativar áudio"),
                                (4, "Configurações"),
                                (5, "Sair"),
                            ] {
                                AppendMenuW(menu, MF_STRING, id, wide(text).as_ptr());
                            }
                            let mut point = POINT::default();
                            GetCursorPos(&mut point);
                            SetForegroundWindow(window);
                            let command = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                                point.x,
                                point.y,
                                0,
                                window,
                                std::ptr::null(),
                            );
                            DestroyMenu(menu);
                            match command {
                                1 => send(DesktopAction::Open),
                                2 => send(DesktopAction::ToggleMute),
                                3 => send(DesktopAction::ToggleDeafen),
                                4 => send(DesktopAction::Settings),
                                5 => send(DesktopAction::Exit),
                                _ => {}
                            }
                        }
                    },
                    _ => {}
                }
                0
            }
            WM_TIMER => {
                if let Ok(mut active) = ACTIVE.lock() {
                    if active
                        .as_ref()
                        .is_some_and(|(at, _)| at.elapsed().as_secs() < 30)
                    {
                        return 0;
                    }
                    *active = None;
                }
                let pending = PENDING.lock().ok().and_then(|mut queue| queue.pop_front());
                if let Some((title, body, target)) = pending {
                    if let Ok(mut active) = ACTIVE.lock() {
                        *active = Some((std::time::Instant::now(), target));
                    }
                    let mut data = icon_data(window);
                    data.uFlags = NIF_INFO;
                    data.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
                    copy_text(&mut data.szInfoTitle, &title);
                    copy_text(&mut data.szInfo, &body);
                    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
                }
                0
            }
            WM_DESTROY => {
                unsafe {
                    Shell_NotifyIconW(NIM_DELETE, &icon_data(window));
                    PostQuitMessage(0);
                }
                0
            }
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    pub(super) fn start(tx: Sender<DesktopAction>, ctx: egui::Context) {
        let _ = REPAINT.set(ctx);
        if ACTIONS.set(tx).is_err() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("desktop-tray".into())
            .spawn(|| unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let class_name = wide("LanvoxNotificationWindow");
                let class = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance,
                    lpszClassName: class_name.as_ptr(),
                    ..std::mem::zeroed()
                };
                if RegisterClassW(&class) == 0 {
                    return;
                }
                let window = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    wide("Lanvox").as_ptr(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if window.is_null() {
                    return;
                }
                WINDOW.store(window as isize, Ordering::SeqCst);
                let keyboard_hook = super::keyboard::install();
                let mut data = icon_data(window);
                data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
                data.uCallbackMessage = CALLBACK;
                data.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
                copy_text(&mut data.szTip, "Lanvox — conversar na sua rede");
                Shell_NotifyIconW(NIM_ADD, &data);
                SetTimer(window, 1, 1000, None);
                let mut message = MSG::default();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                super::keyboard::uninstall(keyboard_hook);
                WINDOW.store(0, Ordering::SeqCst);
            });
    }

    pub(super) fn shutdown() {
        let window = WINDOW.swap(0, Ordering::SeqCst);
        if window != 0 {
            unsafe {
                SendMessageW(window as HWND, WM_CLOSE, 0, 0);
            }
        }
    }

    pub(super) fn notify(title: &str, body: &str) {
        notify_target(title, body, None);
    }

    pub(super) fn notify_target(title: &str, body: &str, target: Option<MessageTarget>) {
        if let Ok(mut queue) = PENDING.lock() {
            if queue.len() >= 8 {
                queue.pop_front();
            }
            queue.push_back((title.to_string(), body.to_string(), target));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn notification_truncation_keeps_utf16_valid_and_terminated() {
            let mut buffer = [0; 5];
            copy_text(&mut buffer, "ab😀c");
            assert_eq!(String::from_utf16(&buffer[..4]).unwrap(), "ab😀");
            assert_eq!(buffer[4], 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_identity_import_never_changes_destination() {
        let path =
            std::env::temp_dir().join(format!("lanvox-identity-test-{}", rand::random::<u64>()));
        std::fs::create_dir(&path).unwrap();
        let destination = path.join("identity.key");
        std::fs::write(&destination, b"original").unwrap();
        assert!(replace_identity(&destination, b"not a key").is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        std::fs::remove_file(destination).unwrap();
        std::fs::remove_dir(path).unwrap();
    }
}
