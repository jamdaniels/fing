use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

use crate::hotkey_config::{get_hotkey_config, HotkeyConfig};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use once_cell::sync::Lazy;
#[cfg(target_os = "windows")]
use rdev::listen;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use rdev::{Event, EventType, Key};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::collections::HashSet;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::{mpsc, Mutex, OnceLock};

static LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(any(target_os = "macos", target_os = "windows"))]
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
#[cfg(any(target_os = "macos", target_os = "windows"))]
static HOTKEY_STATE: Lazy<Mutex<HotkeyState>> = Lazy::new(|| Mutex::new(HotkeyState::default()));
#[cfg(any(target_os = "macos", target_os = "windows"))]
static HOTKEY_EVENT_TX: OnceLock<mpsc::Sender<HotkeyEvent>> = OnceLock::new();

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy)]
enum HotkeyEvent {
    Press,
    Release,
}

pub fn start_hotkey_listener(app: AppHandle) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        start_hotkey_listener_impl(app)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        tracing::warn!("Global hotkey not implemented for this platform");
        let _ = app;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
use rdev::grab;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_hotkey_listener_impl(app: AppHandle) -> Result<(), String> {
    if LISTENER_STARTED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        if !crate::platform::check_accessibility_permission() {
            LISTENER_STARTED.store(false, Ordering::SeqCst);
            return Err("Accessibility permission required for global hotkey".to_string());
        }

        // rdev's keyboard layout handling needs to run TIS APIs on the
        // AppKit main thread. Our grab loop runs on a background thread,
        // so tell rdev to dispatch Unicode lookup work onto the main queue
        // instead of doing it inline on the grab thread.
        rdev::set_is_main_thread(false);
    }

    APP_HANDLE
        .set(app)
        .map_err(|_| "Hotkey listener already initialized".to_string())?;

    HOTKEY_EVENT_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<HotkeyEvent>();

        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                let Some(app) = APP_HANDLE.get() else {
                    continue;
                };

                match event {
                    HotkeyEvent::Press => crate::hotkey::on_key_down(app),
                    HotkeyEvent::Release => crate::hotkey::on_key_up(app),
                }
            }
        });

        tx
    });

    std::thread::spawn(|| {
        #[cfg(target_os = "macos")]
        {
            if let Err(error) = grab(grab_callback) {
                tracing::error!("Global hotkey listener failed: {:?}", error);
            }
        }
        #[cfg(target_os = "windows")]
        {
            // Use listen instead of grab on Windows - WebView2 doesn't properly
            // propagate keyboard events to WH_KEYBOARD_LL hooks when focused.
            // See: https://github.com/tauri-apps/tauri/issues/13919
            if let Err(error) = listen(listen_callback) {
                tracing::error!("Global hotkey listener failed: {:?}", error);
            }
        }
    });

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dispatch_hotkey_event(event: HotkeyEvent) {
    let Some(tx) = HOTKEY_EVENT_TX.get() else {
        if let Some(app) = APP_HANDLE.get() {
            match event {
                HotkeyEvent::Press => crate::hotkey::on_key_down(app),
                HotkeyEvent::Release => crate::hotkey::on_key_up(app),
            }
        }
        return;
    };

    if let Err(error) = tx.send(event) {
        tracing::warn!("Hotkey worker unavailable: {}", error);
        if let Some(app) = APP_HANDLE.get() {
            match event {
                HotkeyEvent::Press => crate::hotkey::on_key_down(app),
                HotkeyEvent::Release => crate::hotkey::on_key_up(app),
            }
        }
    }
}

// macOS uses grab() which can intercept and block key events.
#[cfg(target_os = "macos")]
fn grab_callback(event: Event) -> Option<Event> {
    let test_mode = crate::hotkey::is_onboarding_test_mode();

    let config = match get_hotkey_config() {
        Some(c) => c,
        None => return Some(event),
    };

    let mut state_guard = match HOTKEY_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let state = &mut *state_guard;

    let passthrough = |event: Event| -> Option<Event> {
        if test_mode {
            Some(event)
        } else {
            None
        }
    };

    match event.event_type {
        EventType::KeyPress(key) => {
            if let Some(token) = key_to_token(&key) {
                state.pressed_keys.insert(token);
            }

            if state.hotkey_active {
                return passthrough(event);
            }

            if hotkey_matches(&state.pressed_keys, &config)
                && (test_mode || crate::state::get_state().can_record())
            {
                state.hotkey_active = true;
                dispatch_hotkey_event(HotkeyEvent::Press);
                return passthrough(event);
            }

            Some(event)
        }
        EventType::KeyRelease(key) => {
            let released_token = key_to_token(&key);
            if let Some(token) = released_token.as_ref() {
                state.pressed_keys.remove(token);
            }

            if state.hotkey_active {
                if released_token
                    .as_ref()
                    .is_some_and(|token| config.key_set.contains(token))
                {
                    state.hotkey_active = false;
                    dispatch_hotkey_event(HotkeyEvent::Release);
                }
                return passthrough(event);
            }

            Some(event)
        }
        _ => Some(event),
    }
}

// Windows uses listen() instead of grab() because WebView2 doesn't properly
// propagate keyboard events to WH_KEYBOARD_LL hooks when the window is focused.
// See: https://github.com/tauri-apps/tauri/issues/13919
#[cfg(target_os = "windows")]
fn listen_callback(event: Event) {
    let config = match get_hotkey_config() {
        Some(c) => c,
        None => return,
    };

    let mut state_guard = match HOTKEY_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let state = &mut *state_guard;

    match event.event_type {
        EventType::KeyPress(key) => {
            if let Some(token) = key_to_token(&key) {
                state.pressed_keys.insert(token);
            }

            if !state.hotkey_active
                && hotkey_matches(&state.pressed_keys, &config)
                && crate::state::get_state().can_record()
            {
                state.hotkey_active = true;
                dispatch_hotkey_event(HotkeyEvent::Press);
            }
        }
        EventType::KeyRelease(key) => {
            let released_token = key_to_token(&key);
            if let Some(token) = released_token.as_ref() {
                state.pressed_keys.remove(token);
            }

            if state.hotkey_active
                && released_token
                    .as_ref()
                    .is_some_and(|token| config.key_set.contains(token))
            {
                state.hotkey_active = false;
                dispatch_hotkey_event(HotkeyEvent::Release);
            }
        }
        _ => {}
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct HotkeyState {
    hotkey_active: bool,
    pressed_keys: HashSet<String>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn reset_listener_state() {
    let mut state_guard = match HOTKEY_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    state_guard.hotkey_active = false;
    state_guard.pressed_keys.clear();
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn reset_listener_state() {}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn config_contains_function_free_f_key(config: &HotkeyConfig) -> bool {
    !config.key_set.contains("Function")
        && config
            .key_set
            .iter()
            .any(|token| token.starts_with('F') && token[1..].parse::<u8>().is_ok())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn hotkey_matches(pressed_keys: &HashSet<String>, config: &HotkeyConfig) -> bool {
    if config_contains_function_free_f_key(config) && pressed_keys.contains("Function") {
        let pressed_without_function = pressed_keys
            .iter()
            .filter(|token| token.as_str() != "Function")
            .collect::<HashSet<_>>();
        return pressed_without_function.len() == config.key_set.len()
            && config
                .key_set
                .iter()
                .all(|token| pressed_without_function.contains(token));
    }

    pressed_keys.len() == config.key_set.len()
        && config
            .key_set
            .iter()
            .all(|token| pressed_keys.contains(token))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn key_to_token(key: &Key) -> Option<String> {
    let token = match key {
        Key::Alt => "Alt",
        Key::AltGr => "AltGr",
        Key::Backspace => "Backspace",
        Key::CapsLock => "CapsLock",
        Key::ControlLeft => "ControlLeft",
        Key::ControlRight => "ControlRight",
        Key::Delete => "Delete",
        Key::DownArrow => "DownArrow",
        Key::End => "End",
        Key::Escape => return None,
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::F13 => "F13",
        Key::F14 => "F14",
        Key::F15 => "F15",
        Key::F16 => "F16",
        Key::F17 => "F17",
        Key::F18 => "F18",
        Key::F19 => "F19",
        Key::F20 => "F20",
        Key::F21 => "F21",
        Key::F22 => "F22",
        Key::F23 => "F23",
        Key::F24 => "F24",
        Key::Home => "Home",
        Key::LeftArrow => "LeftArrow",
        Key::MetaLeft => "MetaLeft",
        Key::MetaRight => "MetaRight",
        Key::PageDown => "PageDown",
        Key::PageUp => "PageUp",
        Key::Return => "Return",
        Key::RightArrow => "RightArrow",
        Key::ShiftLeft => "ShiftLeft",
        Key::ShiftRight => "ShiftRight",
        Key::Space => "Space",
        Key::Tab => "Tab",
        Key::UpArrow => "UpArrow",
        Key::PrintScreen => "PrintScreen",
        Key::ScrollLock => "ScrollLock",
        Key::Pause => "Pause",
        Key::NumLock => "NumLock",
        Key::BackQuote => "BackQuote",
        Key::Num0 => "Num0",
        Key::Num1 => "Num1",
        Key::Num2 => "Num2",
        Key::Num3 => "Num3",
        Key::Num4 => "Num4",
        Key::Num5 => "Num5",
        Key::Num6 => "Num6",
        Key::Num7 => "Num7",
        Key::Num8 => "Num8",
        Key::Num9 => "Num9",
        Key::Minus => "Minus",
        Key::Equal => "Equal",
        Key::KeyQ => "KeyQ",
        Key::KeyW => "KeyW",
        Key::KeyE => "KeyE",
        Key::KeyR => "KeyR",
        Key::KeyT => "KeyT",
        Key::KeyY => "KeyY",
        Key::KeyU => "KeyU",
        Key::KeyI => "KeyI",
        Key::KeyO => "KeyO",
        Key::KeyP => "KeyP",
        Key::LeftBracket => "LeftBracket",
        Key::RightBracket => "RightBracket",
        Key::KeyA => "KeyA",
        Key::KeyS => "KeyS",
        Key::KeyD => "KeyD",
        Key::KeyF => "KeyF",
        Key::KeyG => "KeyG",
        Key::KeyH => "KeyH",
        Key::KeyJ => "KeyJ",
        Key::KeyK => "KeyK",
        Key::KeyL => "KeyL",
        Key::SemiColon => "SemiColon",
        Key::Quote => "Quote",
        Key::BackSlash => "Backslash",
        Key::IntlBackslash => "IntlBackslash",
        Key::IntlRo => "IntlRo",
        Key::IntlYen => "IntlYen",
        Key::KanaMode => "KanaMode",
        Key::KeyZ => "KeyZ",
        Key::KeyX => "KeyX",
        Key::KeyC => "KeyC",
        Key::KeyV => "KeyV",
        Key::KeyB => "KeyB",
        Key::KeyN => "KeyN",
        Key::KeyM => "KeyM",
        Key::Comma => "Comma",
        Key::Dot => "Dot",
        Key::Slash => "Slash",
        Key::Insert => "Insert",
        Key::KpReturn => "KpReturn",
        Key::KpMinus => "KpMinus",
        Key::KpPlus => "KpPlus",
        Key::KpMultiply => "KpMultiply",
        Key::KpDivide => "KpDivide",
        Key::KpDecimal => "KpDecimal",
        Key::KpEqual => "KpEqual",
        Key::KpComma => "KpComma",
        Key::Kp0 => "Kp0",
        Key::Kp1 => "Kp1",
        Key::Kp2 => "Kp2",
        Key::Kp3 => "Kp3",
        Key::Kp4 => "Kp4",
        Key::Kp5 => "Kp5",
        Key::Kp6 => "Kp6",
        Key::Kp7 => "Kp7",
        Key::Kp8 => "Kp8",
        Key::Kp9 => "Kp9",
        Key::VolumeUp => "VolumeUp",
        Key::VolumeDown => "VolumeDown",
        Key::VolumeMute => "VolumeMute",
        Key::Lang1 => "Lang1",
        Key::Lang2 => "Lang2",
        Key::Lang3 => "Lang3",
        Key::Lang4 => "Lang4",
        Key::Lang5 => "Lang5",
        Key::Function => "Function",
        Key::Apps => "Apps",
        Key::Cancel => "Cancel",
        Key::Clear => "Clear",
        Key::Kana => "Kana",
        Key::Hangul => "Hangul",
        Key::Junja => "Junja",
        Key::Final => "Final",
        Key::Hanja => "Hanja",
        Key::Hanji => "Hanji",
        Key::Print => "Print",
        Key::Select => "Select",
        Key::Execute => "Execute",
        Key::Help => "Help",
        Key::Sleep => "Sleep",
        Key::Separator => "Separator",
        Key::Unknown(_) | Key::RawKey(_) => return None,
    };

    Some(token.to_string())
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn key_set(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|key| key.to_string()).collect()
    }

    #[test]
    fn matches_exact_key_sets() {
        let config = HotkeyConfig {
            key_set: key_set(&["ControlLeft", "KeyK"]),
            keys: vec!["ControlLeft".to_string(), "KeyK".to_string()],
        };

        assert!(hotkey_matches(&key_set(&["ControlLeft", "KeyK"]), &config));
        assert!(!hotkey_matches(&key_set(&["ControlLeft"]), &config));
        assert!(!hotkey_matches(
            &key_set(&["ControlLeft", "KeyK", "Space"]),
            &config
        ));
    }

    #[test]
    fn ignores_function_for_function_key_hotkeys() {
        let config = HotkeyConfig {
            key_set: key_set(&["F9"]),
            keys: vec!["F9".to_string()],
        };

        assert!(hotkey_matches(&key_set(&["F9"]), &config));
        assert!(hotkey_matches(&key_set(&["Function", "F9"]), &config));
    }

    #[test]
    fn requires_function_when_configured() {
        let config = HotkeyConfig {
            key_set: key_set(&["Function", "F9"]),
            keys: vec!["Function".to_string(), "F9".to_string()],
        };

        assert!(hotkey_matches(&key_set(&["Function", "F9"]), &config));
        assert!(!hotkey_matches(&key_set(&["F9"]), &config));
    }

    #[test]
    fn does_not_map_escape() {
        assert_eq!(key_to_token(&Key::Escape), None);
    }
}
