use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

use crate::hotkey_config::{get_hotkey_config, HotkeyKey};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use once_cell::sync::Lazy;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use rdev::{grab, Event, EventType, Key};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::{Mutex, OnceLock};

static LISTENER_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(any(target_os = "macos", target_os = "windows"))]
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
#[cfg(any(target_os = "macos", target_os = "windows"))]
static HOTKEY_STATE: Lazy<Mutex<HotkeyState>> = Lazy::new(|| Mutex::new(HotkeyState::default()));

pub fn start_hotkey_listener(app: AppHandle) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        return start_hotkey_listener_impl(app);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        tracing::warn!("Global hotkey not implemented for this platform");
        let _ = app;
        Ok(())
    }
}

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

    std::thread::spawn(|| {
        if let Err(error) = grab(grab_callback) {
            tracing::error!("Global hotkey listener failed: {:?}", error);
        }
    });

    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn grab_callback(event: Event) -> Option<Event> {
    let config = match get_hotkey_config() {
        Some(c) => c,
        None => return Some(event),
    };

    let mut state_guard = match HOTKEY_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let state = &mut *state_guard;

    match event.event_type {
        EventType::KeyPress(key) => {
            update_mod_state(&key, true, &mut state.mod_state);

            if is_base_key(&key, &config.key) && modifiers_match(&state.mod_state, &config) {
                if !state.hotkey_active {
                    state.hotkey_active = true;
                    if let Some(app) = APP_HANDLE.get() {
                        crate::hotkey::on_key_down(app);
                    }
                }
                // Swallow base key when used as hotkey
                None
            } else if state.hotkey_active {
                // While the hotkey is held, swallow all other key presses so
                // system shortcuts (Mission Control, etc.) don't interfere.
                None
            } else {
                Some(event)
            }
        }
        EventType::KeyRelease(key) => {
            update_mod_state(&key, false, &mut state.mod_state);

            if is_base_key(&key, &config.key) {
                if state.hotkey_active {
                    state.hotkey_active = false;
                    if let Some(app) = APP_HANDLE.get() {
                        crate::hotkey::on_key_up(app);
                    }
                }
                // Swallow base key release when used as hotkey
                None
            } else {
                // If a required modifier (including Fn) is released while the
                // base key is still held, treat it as hotkey release.
                if state.hotkey_active && !modifiers_match(&state.mod_state, &config) {
                    state.hotkey_active = false;
                    if let Some(app) = APP_HANDLE.get() {
                        crate::hotkey::on_key_up(app);
                    }
                    // Swallow this modifier release as part of the hotkey
                    return None;
                }

                if state.hotkey_active {
                    // Swallow all other key releases while the hotkey is held.
                    None
                } else {
                    Some(event)
                }
            }
        }
        _ => Some(event),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct ModState {
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    fn_down: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct HotkeyState {
    mod_state: ModState,
    hotkey_active: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn update_mod_state(key: &Key, is_down: bool, state: &mut ModState) {
    let value = is_down;
    match key {
        Key::ControlLeft | Key::ControlRight => state.ctrl = value,
        Key::Alt | Key::AltGr => state.alt = value,
        Key::ShiftLeft | Key::ShiftRight => state.shift = value,
        Key::MetaLeft | Key::MetaRight => state.meta = value,
        Key::Function => state.fn_down = value,
        _ => {}
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn modifiers_match(state: &ModState, config: &crate::hotkey_config::HotkeyConfig) -> bool {
    state.ctrl == config.require_ctrl
        && state.alt == config.require_alt
        && state.shift == config.require_shift
        && state.meta == config.require_meta
        && state.fn_down == config.require_fn
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn is_base_key(key: &Key, base: &HotkeyKey) -> bool {
    match base {
        HotkeyKey::Function => matches!(key, Key::Function),
        HotkeyKey::Space => matches!(key, Key::Space),
        HotkeyKey::F(n) => match (n, key) {
            (1, Key::F1) => true,
            (2, Key::F2) => true,
            (3, Key::F3) => true,
            (4, Key::F4) => true,
            (5, Key::F5) => true,
            (6, Key::F6) => true,
            (7, Key::F7) => true,
            (8, Key::F8) => true,
            (9, Key::F9) => true,
            (10, Key::F10) => true,
            (11, Key::F11) => true,
            (12, Key::F12) => true,
            (13, Key::F13) => true,
            (14, Key::F14) => true,
            (15, Key::F15) => true,
            (16, Key::F16) => true,
            (17, Key::F17) => true,
            (18, Key::F18) => true,
            (19, Key::F19) => true,
            (20, Key::F20) => true,
            (21, Key::F21) => true,
            (22, Key::F22) => true,
            (23, Key::F23) => true,
            (24, Key::F24) => true,
            _ => false,
        },
        HotkeyKey::Char(ch) => match char_to_key(*ch) {
            Some(expected) => *key == expected,
            None => false,
        },
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn char_to_key(ch: char) -> Option<Key> {
    match ch {
        'A' => Some(Key::KeyA),
        'B' => Some(Key::KeyB),
        'C' => Some(Key::KeyC),
        'D' => Some(Key::KeyD),
        'E' => Some(Key::KeyE),
        'F' => Some(Key::KeyF),
        'G' => Some(Key::KeyG),
        'H' => Some(Key::KeyH),
        'I' => Some(Key::KeyI),
        'J' => Some(Key::KeyJ),
        'K' => Some(Key::KeyK),
        'L' => Some(Key::KeyL),
        'M' => Some(Key::KeyM),
        'N' => Some(Key::KeyN),
        'O' => Some(Key::KeyO),
        'P' => Some(Key::KeyP),
        'Q' => Some(Key::KeyQ),
        'R' => Some(Key::KeyR),
        'S' => Some(Key::KeyS),
        'T' => Some(Key::KeyT),
        'U' => Some(Key::KeyU),
        'V' => Some(Key::KeyV),
        'W' => Some(Key::KeyW),
        'X' => Some(Key::KeyX),
        'Y' => Some(Key::KeyY),
        'Z' => Some(Key::KeyZ),
        '0' => Some(Key::Num0),
        '1' => Some(Key::Num1),
        '2' => Some(Key::Num2),
        '3' => Some(Key::Num3),
        '4' => Some(Key::Num4),
        '5' => Some(Key::Num5),
        '6' => Some(Key::Num6),
        '7' => Some(Key::Num7),
        '8' => Some(Key::Num8),
        '9' => Some(Key::Num9),
        _ => None,
    }
}
