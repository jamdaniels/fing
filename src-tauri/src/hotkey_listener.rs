use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

use crate::hotkey_config::{get_hotkey_config, HotkeyKey};

#[cfg(any(target_os = "macos", target_os = "windows"))]
use once_cell::sync::Lazy;
#[cfg(target_os = "windows")]
use rdev::listen;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use rdev::{Event, EventType, Key};
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

// macOS uses grab() which can intercept and block key events
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

    // In test mode we run the same detection logic but never swallow events
    // (return Some(event) instead of None). This ensures macOS delivers
    // KeyUp even though we intercepted KeyPress — the indicator window
    // steals focus so we can't rely on WebView JS handlers.
    let passthrough = |event: Event| -> Option<Event> {
        if test_mode {
            Some(event)
        } else {
            None
        }
    };

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
                passthrough(event)
            } else if state.hotkey_active {
                // While the hotkey is held, swallow all other key presses so
                // system shortcuts (Mission Control, etc.) don't interfere.
                passthrough(event)
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
                passthrough(event)
            } else if state.hotkey_active {
                if !modifiers_match(&state.mod_state, &config) {
                    // A required modifier was released while base key still held
                    state.hotkey_active = false;
                    if let Some(app) = APP_HANDLE.get() {
                        crate::hotkey::on_key_up(app);
                    }
                    return passthrough(event);
                }

                if !is_modifier_key(&key) {
                    // Safety net: a non-modifier, non-base-key release while
                    // hotkey is active likely means macOS delivered the base
                    // key release under a different Key variant. Treat it as
                    // the hotkey release to avoid getting stuck.
                    tracing::warn!(
                        "Unexpected KeyRelease {:?} while hotkey active — treating as hotkey release",
                        key
                    );
                    state.hotkey_active = false;
                    if let Some(app) = APP_HANDLE.get() {
                        crate::hotkey::on_key_up(app);
                    }
                }

                passthrough(event)
            } else {
                Some(event)
            }
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
            update_mod_state(&key, true, &mut state.mod_state);

            if is_base_key(&key, &config.key)
                && modifiers_match(&state.mod_state, &config)
                && !state.hotkey_active
            {
                state.hotkey_active = true;
                if let Some(app) = APP_HANDLE.get() {
                    crate::hotkey::on_key_down(app);
                }
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
            } else if state.hotkey_active && !modifiers_match(&state.mod_state, &config) {
                // If a required modifier is released while the base key is still held,
                // treat it as hotkey release.
                state.hotkey_active = false;
                if let Some(app) = APP_HANDLE.get() {
                    crate::hotkey::on_key_up(app);
                }
            }
        }
        _ => {}
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
    // On MacBook keyboards, Fn transforms media keys into F-keys at hardware level.
    // When the base key is an F-key and Fn is NOT explicitly required, ignore Fn state.
    // This allows "F9" hotkey to work whether user presses F9 directly (external kbd)
    // or Fn+F9 (MacBook with media key defaults).
    let fn_matches = if !config.require_fn && matches!(config.key, HotkeyKey::F(_)) {
        // Ignore Fn state for F-key hotkeys that don't explicitly require Fn
        true
    } else {
        state.fn_down == config.require_fn
    };

    state.ctrl == config.require_ctrl
        && state.alt == config.require_alt
        && state.shift == config.require_shift
        && state.meta == config.require_meta
        && fn_matches
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn is_base_key(key: &Key, base: &HotkeyKey) -> bool {
    match base {
        HotkeyKey::Function => matches!(key, Key::Function),
        HotkeyKey::Space => matches!(key, Key::Space),
        HotkeyKey::F(n) => matches!(
            (n, key),
            (1, Key::F1)
                | (2, Key::F2)
                | (3, Key::F3)
                | (4, Key::F4)
                | (5, Key::F5)
                | (6, Key::F6)
                | (7, Key::F7)
                | (8, Key::F8)
                | (9, Key::F9)
                | (10, Key::F10)
                | (11, Key::F11)
                | (12, Key::F12)
                | (13, Key::F13)
                | (14, Key::F14)
                | (15, Key::F15)
                | (16, Key::F16)
                | (17, Key::F17)
                | (18, Key::F18)
                | (19, Key::F19)
                | (20, Key::F20)
                | (21, Key::F21)
                | (22, Key::F22)
                | (23, Key::F23)
                | (24, Key::F24)
        ),
        HotkeyKey::Char(ch) => match char_to_key(*ch) {
            Some(expected) => *key == expected,
            None => false,
        },
    }
}

/// Returns true for modifier keys that should not trigger the safety net release.
#[cfg(target_os = "macos")]
fn is_modifier_key(key: &Key) -> bool {
    matches!(
        key,
        Key::ControlLeft
            | Key::ControlRight
            | Key::Alt
            | Key::AltGr
            | Key::ShiftLeft
            | Key::ShiftRight
            | Key::MetaLeft
            | Key::MetaRight
            | Key::Function
            | Key::CapsLock
            | Key::NumLock
    )
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
