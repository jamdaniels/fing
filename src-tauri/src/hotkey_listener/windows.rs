//! Windows backend: a `WH_KEYBOARD_LL` hook on the listener thread's message loop.
//!
//! Windows silently removes a low-level hook whose callback exceeds the
//! system timeout (common right after resume, when the app is paged out),
//! and there is no API to detect that. The hook is therefore reinstalled
//! after every wake and periodically while idle.

use std::mem::MaybeUninit;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_PACKET, VK_RETURN};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KillTimer, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx,
    HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
};

use super::{handle_key_event, KeyEvent, SleepWatch, ESCAPE_TOKEN};

/// How often the message loop wakes up to check for sleep and refresh the hook.
const HEALTH_CHECK_INTERVAL_MS: u32 = 5_000;
/// Reinstall the hook this often (while idle) in case Windows removed it.
const HOOK_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Windows virtual-key codes to hotkey tokens, plus Escape, which cancels a
/// recording but can't be part of a hotkey. Numpad Enter shares `VK_RETURN`
/// and is distinguished by the extended-key flag in `token_for_key`.
#[rustfmt::skip]
const KEYMAP: &[(u32, &str)] = &[
    (0x03, "Cancel"), (0x08, "Backspace"), (0x09, "Tab"), (0x0C, "Clear"),
    (0x0D, "Return"), (0x13, "Pause"), (0x14, "CapsLock"), (0x15, "Kana"),
    (0x17, "Junja"), (0x18, "Final"), (0x19, "Hanja"), (0x1B, ESCAPE_TOKEN), (0x1C, "Lang2"),
    (0x1D, "Lang1"), (0x20, "Space"), (0x21, "PageUp"), (0x22, "PageDown"),
    (0x23, "End"), (0x24, "Home"), (0x25, "LeftArrow"), (0x26, "UpArrow"),
    (0x27, "RightArrow"), (0x28, "DownArrow"), (0x29, "Select"), (0x2A, "Print"),
    (0x2B, "Execute"), (0x2C, "PrintScreen"), (0x2D, "Insert"), (0x2E, "Delete"),
    (0x2F, "Help"),
    (0x30, "Num0"), (0x31, "Num1"), (0x32, "Num2"), (0x33, "Num3"), (0x34, "Num4"),
    (0x35, "Num5"), (0x36, "Num6"), (0x37, "Num7"), (0x38, "Num8"), (0x39, "Num9"),
    (0x41, "KeyA"), (0x42, "KeyB"), (0x43, "KeyC"), (0x44, "KeyD"), (0x45, "KeyE"),
    (0x46, "KeyF"), (0x47, "KeyG"), (0x48, "KeyH"), (0x49, "KeyI"), (0x4A, "KeyJ"),
    (0x4B, "KeyK"), (0x4C, "KeyL"), (0x4D, "KeyM"), (0x4E, "KeyN"), (0x4F, "KeyO"),
    (0x50, "KeyP"), (0x51, "KeyQ"), (0x52, "KeyR"), (0x53, "KeyS"), (0x54, "KeyT"),
    (0x55, "KeyU"), (0x56, "KeyV"), (0x57, "KeyW"), (0x58, "KeyX"), (0x59, "KeyY"),
    (0x5A, "KeyZ"),
    (0x5B, "MetaLeft"), (0x5C, "MetaRight"), (0x5D, "Apps"), (0x5F, "Sleep"),
    (0x60, "Kp0"), (0x61, "Kp1"), (0x62, "Kp2"), (0x63, "Kp3"), (0x64, "Kp4"),
    (0x65, "Kp5"), (0x66, "Kp6"), (0x67, "Kp7"), (0x68, "Kp8"), (0x69, "Kp9"),
    (0x6A, "KpMultiply"), (0x6B, "KpPlus"), (0x6C, "Separator"), (0x6D, "KpMinus"),
    (0x6E, "KpDecimal"), (0x6F, "KpDivide"),
    (0x70, "F1"), (0x71, "F2"), (0x72, "F3"), (0x73, "F4"), (0x74, "F5"),
    (0x75, "F6"), (0x76, "F7"), (0x77, "F8"), (0x78, "F9"), (0x79, "F10"),
    (0x7A, "F11"), (0x7B, "F12"), (0x7C, "F13"), (0x7D, "F14"), (0x7E, "F15"),
    (0x7F, "F16"), (0x80, "F17"), (0x81, "F18"), (0x82, "F19"), (0x83, "F20"),
    (0x84, "F21"), (0x85, "F22"), (0x86, "F23"), (0x87, "F24"),
    (0x90, "NumLock"), (0x91, "ScrollLock"),
    (0xA0, "ShiftLeft"), (0xA1, "ShiftRight"), (0xA2, "ControlLeft"),
    (0xA3, "ControlRight"), (0xA4, "Alt"), (0xA5, "AltGr"),
    (0xAD, "VolumeMute"), (0xAE, "VolumeDown"), (0xAF, "VolumeUp"),
    (0xBA, "SemiColon"), (0xBB, "Equal"), (0xBC, "Comma"), (0xBD, "Minus"),
    (0xBE, "Dot"), (0xBF, "Slash"), (0xC0, "BackQuote"), (0xDB, "LeftBracket"),
    (0xDC, "Backslash"), (0xDD, "RightBracket"), (0xDE, "Quote"),
    (0xE2, "IntlBackslash"),
];

fn token_for_key(vk_code: u32, extended: bool) -> Option<&'static str> {
    if vk_code == u32::from(VK_RETURN) && extended {
        return Some("KpReturn");
    }

    KEYMAP
        .iter()
        .find(|(code, _)| *code == vk_code)
        .map(|(_, token)| *token)
}

/// Low-level keyboard hooks need no permission on Windows.
pub(super) fn is_permitted() -> bool {
    true
}

/// Whether the OS currently reports the key as held. Inside the hook this is
/// accurate for every key except the one being processed.
pub(super) fn is_key_down(code: u32) -> bool {
    let Ok(vk_code) = i32::try_from(code) else {
        return false;
    };
    unsafe { GetAsyncKeyState(vk_code) < 0 }
}

/// Total time the system has spent asleep since boot, in nanoseconds.
pub(super) fn asleep_ns() -> u64 {
    // The tick count includes sleep; unbiased interrupt time (100ns units) doesn't.
    let mut awake_100ns = 0u64;
    unsafe {
        QueryUnbiasedInterruptTime(&mut awake_100ns);
        GetTickCount64()
            .saturating_mul(1_000_000)
            .saturating_sub(awake_100ns.saturating_mul(100))
    }
}

/// Install the hook and pump messages until it needs to be recreated.
pub(super) fn run() -> Result<&'static str, String> {
    unsafe {
        // Hook callbacks run on this thread; keep it responsive under load so
        // Windows doesn't time the hook out.
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);

        let mut hook = install_hook()?;
        tracing::info!("Hotkey keyboard hook installed");
        // Events may have been missed while no hook was installed.
        super::resync_listener_state();

        let timer = SetTimer(ptr::null_mut(), 0, HEALTH_CHECK_INTERVAL_MS, None);
        if timer == 0 {
            tracing::warn!(
                "Hotkey health timer unavailable (error {}); hook will not self-refresh",
                GetLastError()
            );
        }

        let mut sleep_watch = SleepWatch::new();
        let mut last_refresh = Instant::now();
        let mut message = MaybeUninit::<MSG>::zeroed();

        let result = loop {
            match GetMessageW(message.as_mut_ptr(), ptr::null_mut(), 0, 0) {
                -1 => break Err(format!("GetMessageW failed (error {})", GetLastError())),
                0 => break Ok("message loop received WM_QUIT"),
                _ => {}
            }

            if message.assume_init_ref().message != WM_TIMER {
                continue;
            }

            let woke = sleep_watch.system_slept();
            if woke {
                super::handle_system_wake();
            }

            let refresh_due = last_refresh.elapsed() >= HOOK_REFRESH_INTERVAL && super::is_idle();
            if woke || refresh_due {
                hook = reinstall_hook(hook);
                last_refresh = Instant::now();
            }
        };

        if timer != 0 {
            KillTimer(ptr::null_mut(), timer);
        }
        UnhookWindowsHookEx(hook);

        result
    }
}

unsafe fn install_hook() -> Result<HHOOK, String> {
    let hook = SetWindowsHookExW(
        WH_KEYBOARD_LL,
        Some(keyboard_hook),
        GetModuleHandleW(ptr::null()),
        0,
    );
    if hook.is_null() {
        return Err(format!(
            "SetWindowsHookExW failed (error {})",
            GetLastError()
        ));
    }
    Ok(hook)
}

/// Swap in a fresh hook. The new hook is installed before the old one is
/// removed so no key events fall through the gap.
unsafe fn reinstall_hook(old_hook: HHOOK) -> HHOOK {
    let new_hook = match install_hook() {
        Ok(hook) => hook,
        Err(error) => {
            tracing::warn!("Failed to refresh hotkey keyboard hook: {}", error);
            return old_hook;
        }
    };

    if UnhookWindowsHookEx(old_hook) == 0 {
        // The old handle is invalid when Windows already removed the hook.
        tracing::warn!(
            "Previous hotkey keyboard hook was already gone (error {}), likely removed by Windows",
            GetLastError()
        );
    }

    // Events may have been missed if the old hook was dead.
    super::resync_listener_state();
    new_hook
}

/// Listen-only: events are always passed on, because WebView2 doesn't
/// propagate keyboard events to blocking `WH_KEYBOARD_LL` hooks (see
/// tauri-apps/tauri#13919).
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam as *const KBDLLHOOKSTRUCT);
        if let Some(event) = convert(wparam as u32, info) {
            handle_key_event(event);
        }
    }

    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

fn convert(message: u32, info: &KBDLLHOOKSTRUCT) -> Option<KeyEvent> {
    let is_press = match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return None,
    };

    // VK_PACKET carries Unicode text injected via SendInput (including our
    // own paste), not a physical key.
    if info.vkCode == u32::from(VK_PACKET) {
        return None;
    }

    let token = token_for_key(info.vkCode, info.flags & LLKHF_EXTENDED != 0)?;
    Some(KeyEvent {
        token,
        code: info.vkCode,
        is_press,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;

    #[test]
    fn keymap_tokens_are_valid_and_unique() {
        crate::hotkey_listener::tests::assert_keymap_is_consistent(KEYMAP);
    }

    #[test]
    fn numpad_enter_uses_extended_flag() {
        assert_eq!(token_for_key(u32::from(VK_RETURN), false), Some("Return"));
        assert_eq!(token_for_key(u32::from(VK_RETURN), true), Some("KpReturn"));
    }

    #[test]
    fn escape_maps_to_cancel_token() {
        assert_eq!(
            token_for_key(u32::from(VK_ESCAPE), false),
            Some(ESCAPE_TOKEN)
        );
    }

    #[test]
    fn ignores_injected_unicode_packets() {
        let info = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_PACKET),
            scanCode: u32::from('a'),
            flags: 0,
            time: 0,
            dwExtraInfo: 0,
        };
        assert!(convert(WM_KEYDOWN, &info).is_none());
    }
}
