// macOS-specific platform code

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{OnceLock, RwLock};
use tauri::AppHandle;

// Type aliases for CoreFoundation/CoreGraphics types
type CFMachPortRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: *const c_void);
    fn CFRunLoopRun();
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: i64,
    ) -> CFRunLoopSourceRef;
    fn CFRelease(cf: *mut c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(stateID: i32) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(
        source: *mut c_void,
        virtualKey: u16,
        keyDown: bool,
    ) -> *mut c_void;
    fn CGEventPost(tap: i32, event: *mut c_void);
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventKeyboardSetUnicodeString(
        event: *mut c_void,
        stringLength: std::ffi::c_ulong,
        unicodeString: *const u16,
    );
}

// CGEventFlags
const K_CG_EVENT_FLAG_MASK_SHIFT: u64 = 1 << 17;
const K_CG_EVENT_FLAG_MASK_CONTROL: u64 = 1 << 18;
const K_CG_EVENT_FLAG_MASK_OPTION: u64 = 1 << 19;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 1 << 20;

// Virtual key codes
const K_VK_F8: i64 = 100;

// CGEventTapLocation
const K_CG_HID_EVENT_TAP: i32 = 0;

// CGEventTapPlacement
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;

// CGEventType
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;

// CGEventMask
const K_CG_EVENT_MASK_FOR_KEY_DOWN: u64 = 1 << K_CG_EVENT_KEY_DOWN;
const K_CG_EVENT_MASK_FOR_KEY_UP: u64 = 1 << K_CG_EVENT_KEY_UP;
const K_CG_EVENT_MASK_FOR_FLAGS_CHANGED: u64 = 1 << K_CG_EVENT_FLAGS_CHANGED;

// CGEventField
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;

// Fn key flag (NX_SECONDARYFNMASK)
const K_CG_EVENT_FLAG_MASK_FN: u64 = 1 << 23;

// Special keycode for Fn key (not a real keycode, used internally)
const K_VK_FN: i64 = -1;

// kCFRunLoopCommonModes
extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
}

// CGEventSourceStateID
const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: i32 = 1;

// Global app handle storage
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

// Remember the app that was frontmost when the hotkey was pressed, so we can
// restore focus after OS-level shortcuts (e.g. Dock, Mission Control) fire.
static HOTKEY_FRONT_APP: OnceLock<RwLock<Option<String>>> = OnceLock::new();

fn get_hotkey_front_app_slot() -> &'static RwLock<Option<String>> {
    HOTKEY_FRONT_APP.get_or_init(|| RwLock::new(None))
}

fn remember_front_app_for_hotkey() {
    if let Some(id) = get_frontmost_app() {
        if let Ok(mut slot) = get_hotkey_front_app_slot().write() {
            *slot = Some(id);
        }
    }
}

fn restore_front_app_for_hotkey() {
    let bundle_id = {
        if let Ok(slot) = get_hotkey_front_app_slot().read() {
            slot.clone()
        } else {
            None
        }
    };

    if let Some(id) = bundle_id {
        if let Err(e) = activate_app(&id) {
            tracing::warn!("Failed to restore frontmost app {}: {}", id, e);
        }
    }
}

/// Parsed hotkey configuration
#[derive(Clone)]
struct ParsedHotkey {
    keycode: Option<i64>,    // None for Fn-only
    required_modifiers: u64, // CGEventFlags mask (Ctrl, Option, Shift, Cmd)
    uses_fn: bool,           // Whether Fn key is part of the combination
}

impl Default for ParsedHotkey {
    fn default() -> Self {
        Self {
            keycode: Some(K_VK_F8),
            required_modifiers: 0,
            uses_fn: false,
        }
    }
}

// Current hotkey configuration
static CURRENT_HOTKEY: OnceLock<RwLock<ParsedHotkey>> = OnceLock::new();

fn get_hotkey_config() -> &'static RwLock<ParsedHotkey> {
    CURRENT_HOTKEY.get_or_init(|| RwLock::new(ParsedHotkey::default()))
}

// Track Fn key state (for detecting press/release)
static FN_KEY_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// Track if hotkey is currently pressed (for combinations)
static HOTKEY_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Map a key name to macOS virtual keycode
pub fn key_name_to_keycode(key: &str) -> Option<i64> {
    match key.to_uppercase().as_str() {
        "FN" => Some(K_VK_FN), // Special case for Fn key
        "F1" => Some(122),
        "F2" => Some(120),
        "F3" => Some(99),
        "F4" => Some(118),
        "F5" => Some(96),
        "F6" => Some(97),
        "F7" => Some(98),
        "F8" => Some(100),
        "F9" => Some(101),
        "F10" => Some(109),
        "F11" => Some(103),
        "F12" => Some(111),
        "F13" => Some(105),
        "F14" => Some(107),
        "F15" => Some(113),
        "A" => Some(0),
        "B" => Some(11),
        "C" => Some(8),
        "D" => Some(2),
        "E" => Some(14),
        "F" => Some(3),
        "G" => Some(5),
        "H" => Some(4),
        "I" => Some(34),
        "J" => Some(38),
        "K" => Some(40),
        "L" => Some(37),
        "M" => Some(46),
        "N" => Some(45),
        "O" => Some(31),
        "P" => Some(35),
        "Q" => Some(12),
        "R" => Some(15),
        "S" => Some(1),
        "T" => Some(17),
        "U" => Some(32),
        "V" => Some(9),
        "W" => Some(13),
        "X" => Some(7),
        "Y" => Some(16),
        "Z" => Some(6),
        "0" => Some(29),
        "1" => Some(18),
        "2" => Some(19),
        "3" => Some(20),
        "4" => Some(21),
        "5" => Some(23),
        "6" => Some(22),
        "7" => Some(26),
        "8" => Some(28),
        "9" => Some(25),
        "SPACE" => Some(49),
        _ => None,
    }
}

/// Parse modifier name to CGEventFlags mask
fn modifier_name_to_flag(name: &str) -> Option<u64> {
    match name.to_uppercase().as_str() {
        "CTRL" | "CONTROL" => Some(K_CG_EVENT_FLAG_MASK_CONTROL),
        "OPTION" | "ALT" => Some(K_CG_EVENT_FLAG_MASK_OPTION),
        "SHIFT" => Some(K_CG_EVENT_FLAG_MASK_SHIFT),
        "CMD" | "COMMAND" | "META" => Some(K_CG_EVENT_FLAG_MASK_COMMAND),
        _ => None,
    }
}

/// Maximum allowed hotkey string length
const MAX_HOTKEY_LENGTH: usize = 50;

/// Maximum number of parts in a hotkey combination
const MAX_HOTKEY_PARTS: usize = 5;

/// Update the current hotkey (supports combinations like "Option+Space")
pub fn set_hotkey(key: &str) -> Result<(), String> {
    // Validate total length
    if key.len() > MAX_HOTKEY_LENGTH {
        return Err(format!("Hotkey too long (max {} chars)", MAX_HOTKEY_LENGTH));
    }

    // Validate characters - only allow alphanumeric, +, and space
    if !key
        .chars()
        .all(|c| c.is_alphanumeric() || c == '+' || c == ' ')
    {
        return Err("Hotkey contains invalid characters".to_string());
    }

    let parts: Vec<&str> = key.split('+').collect();

    // Validate part count
    if parts.len() > MAX_HOTKEY_PARTS {
        return Err(format!(
            "Too many keys in combination (max {})",
            MAX_HOTKEY_PARTS
        ));
    }

    let mut required_modifiers: u64 = 0;
    let mut keycode: Option<i64> = None;
    let mut uses_fn = false;

    for part in parts {
        let part = part.trim();

        // Validate individual part length
        if part.len() > 10 {
            return Err(format!("Invalid key name: {}", part));
        }

        // Check if it's a modifier
        if let Some(flag) = modifier_name_to_flag(part) {
            required_modifiers |= flag;
            continue;
        }

        // Check if it's Fn key
        if part.to_uppercase() == "FN" {
            uses_fn = true;
            continue;
        }

        // It's the base key - validate against allowlist
        if keycode.is_some() {
            return Err(format!("Multiple base keys in hotkey: {}", key));
        }
        keycode = Some(key_name_to_keycode(part).ok_or_else(|| format!("Unknown key: {}", part))?);
    }

    // Fn-only mode: no base key and no modifiers
    if keycode.is_none() && required_modifiers == 0 && !uses_fn {
        return Err("No valid key in hotkey".to_string());
    }

    let config = ParsedHotkey {
        keycode,
        required_modifiers,
        uses_fn,
    };

    // Store the config
    let hotkey_lock = get_hotkey_config();
    let mut hotkey = hotkey_lock
        .write()
        .map_err(|e| format!("Lock error: {}", e))?;
    *hotkey = config;

    // Reset state
    HOTKEY_ACTIVE.store(false, Ordering::SeqCst);
    FN_KEY_DOWN.store(false, Ordering::SeqCst);

    tracing::info!(
        "Hotkey updated to: {} (keycode: {:?}, modifiers: 0x{:x}, fn: {})",
        key,
        keycode,
        required_modifiers,
        uses_fn
    );
    Ok(())
}

/// Get a copy of the current hotkey configuration
fn get_current_hotkey() -> ParsedHotkey {
    get_hotkey_config()
        .read()
        .map(|h| h.clone())
        .unwrap_or_default()
}

/// Check if the current modifier flags match the required modifiers exactly
fn modifiers_match(current_flags: u64, required: u64) -> bool {
    // Mask to extract only the modifier bits we care about
    let modifier_mask = K_CG_EVENT_FLAG_MASK_SHIFT
        | K_CG_EVENT_FLAG_MASK_CONTROL
        | K_CG_EVENT_FLAG_MASK_OPTION
        | K_CG_EVENT_FLAG_MASK_COMMAND;

    let current_modifiers = current_flags & modifier_mask;
    current_modifiers == required
}

// CGEventTap callback
extern "C" fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    let config = get_current_hotkey();
    let flags = unsafe { CGEventGetFlags(event) };
    let fn_pressed = (flags & K_CG_EVENT_FLAG_MASK_FN) != 0;

    // Handle Fn-only mode (no base key, just Fn)
    if config.keycode.is_none() && config.uses_fn && config.required_modifiers == 0 {
        // We treat Fn as the hotkey itself. While it is held, we swallow
        // all related keyboard events so system features bound to Fn do not fire.
        if event_type == K_CG_EVENT_FLAGS_CHANGED {
            let was_pressed = FN_KEY_DOWN.load(Ordering::SeqCst);

            if fn_pressed && !was_pressed {
                FN_KEY_DOWN.store(true, Ordering::SeqCst);
                remember_front_app_for_hotkey();
                if let Some(app) = APP_HANDLE.get() {
                    crate::hotkey::on_key_down(app);
                }
            } else if !fn_pressed && was_pressed {
                FN_KEY_DOWN.store(false, Ordering::SeqCst);
                if let Some(app) = APP_HANDLE.get() {
                    crate::hotkey::on_key_up(app);
                }
                // Restore the app that was frontmost when Fn was pressed
                restore_front_app_for_hotkey();
            }

            // Swallow all Fn-related flags changes in Fn-only mode
            return std::ptr::null_mut();
        }

        // While Fn is held as the hotkey, swallow all other key events too
        if FN_KEY_DOWN.load(Ordering::SeqCst) {
            return std::ptr::null_mut();
        }

        // Otherwise (Fn not involved), pass event through
        return event;
    }

    // Handle hotkeys with a base key (with optional modifiers and/or Fn)
    if let Some(target_keycode) = config.keycode {
        let keycode = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };

        if keycode == target_keycode {
            // Check if modifiers match exactly
            let modifiers_ok = modifiers_match(flags, config.required_modifiers);
            // Check Fn requirement
            let fn_ok = !config.uses_fn || fn_pressed;

            if modifiers_ok && fn_ok {
                let was_active = HOTKEY_ACTIVE.load(Ordering::SeqCst);

                match event_type {
                    K_CG_EVENT_KEY_DOWN => {
                        if !was_active {
                            HOTKEY_ACTIVE.store(true, Ordering::SeqCst);
                            remember_front_app_for_hotkey();
                            if let Some(app) = APP_HANDLE.get() {
                                crate::hotkey::on_key_down(app);
                            }
                        }
                        // Swallow the hotkey key-down event
                        return std::ptr::null_mut();
                    }
                    K_CG_EVENT_KEY_UP => {
                        if was_active {
                            HOTKEY_ACTIVE.store(false, Ordering::SeqCst);
                            if let Some(app) = APP_HANDLE.get() {
                                crate::hotkey::on_key_up(app);
                            }
                            // Restore the app that was frontmost when the hotkey was pressed
                            restore_front_app_for_hotkey();
                        }
                        // Swallow the hotkey key-up event
                        return std::ptr::null_mut();
                    }
                    _ => {}
                }
            }
        }
    }

    // Also handle modifier release while hotkey is active (user releases modifier before key)
    if HOTKEY_ACTIVE.load(Ordering::SeqCst) && event_type == K_CG_EVENT_FLAGS_CHANGED {
        let modifiers_ok = modifiers_match(flags, config.required_modifiers);
        let fn_ok = !config.uses_fn || fn_pressed;

        if !modifiers_ok || !fn_ok {
            HOTKEY_ACTIVE.store(false, Ordering::SeqCst);
            if let Some(app) = APP_HANDLE.get() {
                crate::hotkey::on_key_up(app);
            }
        }
    }

    event
}

pub fn check_accessibility_permission() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn request_accessibility_permission() -> bool {
    // On macOS, we can open System Preferences to the accessibility pane
    // The actual permission request happens automatically when we try to use accessibility features
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();

    check_accessibility_permission()
}

/// Open System Preferences to the Microphone privacy pane
pub fn request_microphone_permission() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        .spawn();
}

/// Check if microphone permission is granted by trying to capture audio
pub fn check_microphone_permission() -> String {
    use crate::audio::AudioCapture;

    // Try to actually capture audio - this triggers the permission prompt
    let mut capture = AudioCapture::new();
    match capture.test_microphone() {
        Ok(test) => {
            // If we got audio data, permission is granted
            // If buffer is empty or no audio received, permission might be denied
            if test.is_receiving_audio || test.peak_level > 0.0 {
                "granted".to_string()
            } else {
                // Got no audio - could be permission denied or just silence
                // Check if we have any devices at all
                let devices = AudioCapture::list_devices();
                if devices.is_empty() {
                    "denied".to_string()
                } else {
                    // Have devices but no audio - likely need to grant permission
                    // Return "prompt" to indicate user should try granting
                    "prompt".to_string()
                }
            }
        }
        Err(e) => {
            tracing::warn!("Microphone check failed: {}", e);
            "denied".to_string()
        }
    }
}

/// Filter text to printable characters only (security: prevent control char injection)
fn filter_printable(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Type text directly using CGEventKeyboardSetUnicodeString (no clipboard)
pub fn type_text(text: &str) -> Result<(), String> {
    if !check_accessibility_permission() {
        return Err("Accessibility permission required".to_string());
    }

    let filtered = filter_printable(text);
    let utf16: Vec<u16> = filtered.encode_utf16().collect();

    if utf16.is_empty() {
        return Ok(());
    }

    unsafe {
        let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE);
        if source.is_null() {
            return Err("Failed to create event source".to_string());
        }

        // Key down event with full text
        let key_down = CGEventCreateKeyboardEvent(source, 0, true);
        if key_down.is_null() {
            CFRelease(source);
            return Err("Failed to create keyboard event".to_string());
        }

        CGEventKeyboardSetUnicodeString(key_down, utf16.len() as std::ffi::c_ulong, utf16.as_ptr());
        CGEventPost(K_CG_HID_EVENT_TAP, key_down);
        CFRelease(key_down);

        // Key up event
        let key_up = CGEventCreateKeyboardEvent(source, 0, false);
        if !key_up.is_null() {
            CGEventPost(K_CG_HID_EVENT_TAP, key_up);
            CFRelease(key_up);
        }

        CFRelease(source);
    }

    Ok(())
}

/// Escape a string for safe use in AppleScript double-quoted strings
fn escape_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Get the application bundle path
fn get_app_path() -> Result<String, String> {
    let exe_path = std::env::current_exe().map_err(|e| format!("Failed to get exe path: {}", e))?;

    // Navigate up to find the .app bundle
    // Typically: /Applications/Fing.app/Contents/MacOS/fing
    let mut path = exe_path.as_path();
    while let Some(parent) = path.parent() {
        if path.extension().map_or(false, |ext| ext == "app") {
            return Ok(path.to_string_lossy().to_string());
        }
        path = parent;
    }

    // Fallback to the executable path if not in a bundle
    Ok(exe_path.to_string_lossy().to_string())
}

/// Enable auto-start on login using macOS Login Items
pub fn enable_auto_start() -> Result<(), String> {
    let app_path = get_app_path()?;
    let escaped_path = escape_applescript_string(&app_path);

    let script = format!(
        r#"tell application "System Events"
            if not (exists login item "Fing") then
                make login item at end with properties {{path:"{}", hidden:false}}
            end if
        end tell"#,
        escaped_path
    );

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to add login item: {}", stderr));
    }

    tracing::info!("Auto-start enabled for: {}", app_path);
    Ok(())
}

/// Disable auto-start on login
pub fn disable_auto_start() -> Result<(), String> {
    let script = r#"tell application "System Events"
        if exists login item "Fing" then
            delete login item "Fing"
        end if
    end tell"#;

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to remove login item: {}", stderr));
    }

    tracing::info!("Auto-start disabled");
    Ok(())
}

/// Check if auto-start is enabled
pub fn is_auto_start_enabled() -> bool {
    let script = r#"tell application "System Events"
        return exists login item "Fing"
    end tell"#;

    let output = match std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("Failed to check login item: {}", e);
            return false;
        }
    };

    let result = String::from_utf8_lossy(&output.stdout);
    result.trim() == "true"
}

/// Get the bundle identifier of the frontmost application
pub fn get_frontmost_app() -> Option<String> {
    let script = r#"tell application "System Events"
        set frontApp to first application process whose frontmost is true
        return bundle identifier of frontApp
    end tell"#;

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .ok()?;

    if !output.status.success() {
        tracing::warn!("Failed to get frontmost app");
        return None;
    }

    let bundle_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if bundle_id.is_empty() {
        None
    } else {
        tracing::debug!("Captured frontmost app: {}", bundle_id);
        Some(bundle_id)
    }
}

/// Activate an application by bundle identifier
pub fn activate_app(bundle_id: &str) -> Result<(), String> {
    let escaped_id = escape_applescript_string(bundle_id);
    let script = format!(r#"tell application id "{}" to activate"#, escaped_id);

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("Failed to run osascript: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to activate app {}: {}", bundle_id, stderr));
    }

    tracing::debug!("Activated app: {}", bundle_id);
    Ok(())
}

/// Register global hotkey using CGEventTap
/// Note: Does NOT open System Preferences if permission is missing.
/// Use request_accessibility_permission() to prompt the user explicitly.
pub fn register_global_hotkey(app: AppHandle) -> Result<(), String> {
    // Check accessibility permission first - but don't open System Preferences
    if !check_accessibility_permission() {
        tracing::warn!("Accessibility permission not granted - hotkey registration skipped");
        return Err("Accessibility permission required for global hotkey".to_string());
    }

    // Store app handle globally
    APP_HANDLE
        .set(app)
        .map_err(|_| "App handle already set".to_string())?;

    // Spawn background thread for event tap run loop
    std::thread::spawn(|| {
        unsafe {
            // Create event tap for key events and flags changed (for Fn key)
            let event_mask = K_CG_EVENT_MASK_FOR_KEY_DOWN
                | K_CG_EVENT_MASK_FOR_KEY_UP
                | K_CG_EVENT_MASK_FOR_FLAGS_CHANGED;

            let event_tap = CGEventTapCreate(
                K_CG_HID_EVENT_TAP as u32,
                K_CG_HEAD_INSERT_EVENT_TAP,
                0,
                event_mask,
                event_tap_callback,
                std::ptr::null_mut(),
            );

            if event_tap.is_null() {
                tracing::error!(
                    "Failed to create event tap - accessibility permission may be required"
                );
                return;
            }

            // Create run loop source
            let run_loop_source = CFMachPortCreateRunLoopSource(std::ptr::null(), event_tap, 0);
            if run_loop_source.is_null() {
                tracing::error!("Failed to create run loop source");
                CFRelease(event_tap);
                return;
            }

            // Add to current run loop
            let run_loop = CFRunLoopGetCurrent();
            CFRunLoopAddSource(run_loop, run_loop_source, kCFRunLoopCommonModes);

            // Enable the event tap
            CGEventTapEnable(event_tap, true);

            tracing::info!("Global hotkey (F8) registered via CGEventTap");

            // Run the loop - this blocks
            CFRunLoopRun();

            // Cleanup (won't reach unless run loop is stopped)
            CFRelease(run_loop_source);
            CFRelease(event_tap);
        }
    });

    Ok(())
}
