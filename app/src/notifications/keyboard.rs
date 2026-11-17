//! Windows keyboard input shared by global shortcuts and the shortcut recorder.
use crate::settings::Hotkey;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static INSTALLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Keyboard> = Mutex::new(Keyboard::new());

struct Keyboard {
    down: [bool; 256],
    wanted: [Hotkey; 2],
    recording: bool,
    candidate: Option<Hotkey>,
    captured: Option<Hotkey>,
}

impl Keyboard {
    const fn new() -> Self {
        Self {
            down: [false; 256],
            wanted: [Hotkey::none(); 2],
            recording: false,
            candidate: None,
            captured: None,
        }
    }

    fn modifiers(&self, vk: u32) -> (bool, bool, bool) {
        let held = |a: usize, b: usize| self.down[a] || self.down[b];
        // A modifier used as the main key must not also require itself.
        (
            !matches!(vk, 0xA2..=0xA3) && held(0xA2, 0xA3),
            !matches!(vk, 0xA0..=0xA1) && held(0xA0, 0xA1),
            !matches!(vk, 0xA4..=0xA5) && held(0xA4, 0xA5),
        )
    }

    fn event(&mut self, vk: u32, pressed: bool) -> [bool; 2] {
        let Some(previous) = self.down.get_mut(vk as usize) else {
            return [false; 2];
        };
        let repeated = *previous == pressed;
        *previous = pressed;
        if repeated {
            return [false; 2];
        }
        let (ctrl, shift, alt) = self.modifiers(vk);
        if self.recording {
            if self.captured.is_some() {
                return [false; 2];
            }
            if pressed {
                if vk == 0x1B {
                    self.captured = Some(Hotkey::none());
                } else if matches!(vk, 0xA0..=0xA5) {
                    // Standalone physical modifiers also work while another
                    // modifier is held (including AltGr's companion Ctrl).
                    self.candidate = Some(Hotkey::new(false, false, false, vk));
                } else if (0x70..=0x87).contains(&vk) || ctrl || alt {
                    self.captured = Some(Hotkey::new(ctrl, shift, alt, vk));
                    self.candidate = None;
                }
            } else if self.candidate.is_some_and(|key| key.vk == vk) {
                self.captured = self.candidate.take();
            }
            return [false; 2];
        }
        self.wanted.map(|key| {
            pressed
                && key.is_set()
                && key.vk == vk
                && ((matches!(vk, 0xA0..=0xA5) && !key.ctrl && !key.shift && !key.alt)
                    || (key.ctrl, key.shift, key.alt) == (ctrl, shift, alt))
        })
    }
}

pub(super) fn set_hotkeys(keys: [Hotkey; 2]) {
    if let Ok(mut state) = STATE.lock() {
        state.wanted = keys;
    }
}

pub(super) fn status() -> (bool, bool) {
    let installed = INSTALLED.load(Ordering::Acquire);
    STATE
        .lock()
        .map(|s| {
            (
                installed && s.wanted[0].is_set(),
                installed && s.wanted[1].is_set(),
            )
        })
        .unwrap_or_default()
}

pub(super) fn record(active: bool) {
    if let Ok(mut state) = STATE.lock() {
        state.recording = active;
        state.candidate = None;
        state.captured = None;
    }
}

pub(super) fn take_recorded() -> Option<Hotkey> {
    STATE.lock().ok()?.captured.take()
}

unsafe extern "system" fn procedure(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && matches!(
            wparam as u32,
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
        )
    {
        let input = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // Shortcuts respond to physical input rather than software-generated keys.
        if input.flags & LLKHF_INJECTED == 0 {
            let pressed = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let actions = if let Ok(mut state) = STATE.lock() {
                let can_record = if state.recording {
                    let mut pid = 0;
                    unsafe {
                        GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
                    }
                    pid == unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() }
                } else {
                    true
                };
                if can_record {
                    state.event(input.vkCode, pressed)
                } else {
                    state.candidate = None;
                    state.captured = None;
                    if let Some(down) = state.down.get_mut(input.vkCode as usize) {
                        *down = pressed;
                    }
                    [false; 2]
                }
            } else {
                [false; 2]
            };
            for (active, action) in actions.into_iter().zip([
                super::DesktopAction::ToggleMute,
                super::DesktopAction::ToggleDeafen,
            ]) {
                if active {
                    super::native::send(action);
                }
            }
        }
    }
    // Observe keys without consuming input from other applications.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

pub(super) unsafe fn install() -> HHOOK {
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(procedure),
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()),
            0,
        )
    };
    INSTALLED.store(!hook.is_null(), Ordering::Release);
    hook
}

pub(super) unsafe fn uninstall(hook: HHOOK) {
    INSTALLED.store(false, Ordering::Release);
    if !hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(hook);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_modifiers_are_distinct_and_do_not_repeat() {
        let mut state = Keyboard::new();
        state.wanted = [
            Hotkey::new(false, false, false, 0xA3),
            Hotkey::new(false, false, false, 0xA5),
        ];
        assert_eq!(state.event(0xA2, true), [false, false]);
        state.event(0xA2, false);
        assert_eq!(state.event(0xA3, true), [true, false]);
        assert_eq!(state.event(0xA3, true), [false, false]);
        assert_eq!(state.event(0xA3, false), [false, false]);
        assert_eq!(state.event(0xA5, true), [false, true]);
    }

    #[test]
    fn recorder_waits_for_modifier_release_and_prefers_combinations() {
        let mut state = Keyboard::new();
        state.recording = true;
        state.event(0xA3, true);
        assert_eq!(state.captured, None);
        state.event(0xA3, false);
        assert_eq!(
            state.captured.take(),
            Some(Hotkey::new(false, false, false, 0xA3))
        );
        state.event(0xA2, true);
        state.event(0x4D, true);
        let combination = Some(Hotkey::new(true, false, false, 0x4D));
        assert_eq!(state.captured, combination);
        state.event(0xA2, false);
        assert_eq!(state.captured, combination);
    }

    #[test]
    fn combinations_require_exact_modifiers_and_only_fire_on_press() {
        let mut state = Keyboard::new();
        state.wanted[0] = Hotkey::new(true, true, false, 0x4D);
        state.event(0xA3, true);
        assert_eq!(state.event(0x4D, true), [false; 2]);
        state.event(0x4D, false);
        state.event(0xA1, true);
        assert_eq!(state.event(0x4D, true), [true, false]);
        assert_eq!(state.event(0x4D, true), [false; 2]);
        assert_eq!(state.event(0x4D, false), [false; 2]);
    }

    #[test]
    fn altgr_can_be_recorded_and_used_with_its_companion_control() {
        let mut state = Keyboard::new();
        state.recording = true;
        state.event(0xA2, true);
        state.event(0xA5, true);
        state.event(0xA5, false);
        let key = Hotkey::new(false, false, false, 0xA5);
        assert_eq!(state.captured, Some(key));
        state.recording = false;
        state.wanted[0] = key;
        assert_eq!(state.event(0xA5, true), [true, false]);
    }

    #[test]
    fn recording_suppresses_actions_and_escape_cancels() {
        let mut state = Keyboard::new();
        state.wanted[0] = Hotkey::new(false, false, false, 0x70);
        state.recording = true;
        assert_eq!(state.event(0x70, true), [false; 2]);
        state.captured = None;
        state.event(0x1B, true);
        assert_eq!(state.captured, Some(Hotkey::none()));
    }
}
