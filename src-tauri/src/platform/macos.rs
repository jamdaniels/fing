// macOS-specific platform code

use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;
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
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
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
}

// CGEventFlags
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 1 << 20;

// Virtual key codes
const K_VK_V: u16 = 9;
const K_VK_F8: i64 = 100;

// CGEventTapLocation
const K_CG_HID_EVENT_TAP: i32 = 0;
const K_CG_SESSION_EVENT_TAP: u32 = 1;

// CGEventTapPlacement
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;

// CGEventTapOptions
const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;

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

// Current hotkey keycode (default F8 = 100)
static CURRENT_HOTKEY: AtomicI64 = AtomicI64::new(K_VK_F8);

// Track Fn key state (for detecting press/release)
static FN_KEY_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// Update the current hotkey
pub fn set_hotkey(key: &str) -> Result<(), String> {
    let keycode = key_name_to_keycode(key).ok_or_else(|| format!("Unknown key: {}", key))?;
    CURRENT_HOTKEY.store(keycode, Ordering::SeqCst);
    tracing::info!("Hotkey updated to {} (keycode {})", key, keycode);
    Ok(())
}

/// Get the current hotkey keycode
pub fn get_current_hotkey() -> i64 {
    CURRENT_HOTKEY.load(Ordering::SeqCst)
}

// CGEventTap callback
extern "C" fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    let current_hotkey = get_current_hotkey();

    // Handle Fn key specially via flags changed event
    if current_hotkey == K_VK_FN {
        if event_type == K_CG_EVENT_FLAGS_CHANGED {
            let flags = unsafe { CGEventGetFlags(event) };
            let fn_pressed = (flags & K_CG_EVENT_FLAG_MASK_FN) != 0;
            let was_pressed = FN_KEY_DOWN.load(std::sync::atomic::Ordering::SeqCst);

            if fn_pressed && !was_pressed {
                // Fn key pressed
                FN_KEY_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(app) = APP_HANDLE.get() {
                    crate::hotkey::on_key_down(app);
                }
            } else if !fn_pressed && was_pressed {
                // Fn key released
                FN_KEY_DOWN.store(false, std::sync::atomic::Ordering::SeqCst);
                if let Some(app) = APP_HANDLE.get() {
                    crate::hotkey::on_key_up(app);
                }
            }
        }
    } else {
        // Handle regular key events
        let keycode = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };

        if keycode == current_hotkey {
            if let Some(app) = APP_HANDLE.get() {
                match event_type {
                    K_CG_EVENT_KEY_DOWN => {
                        crate::hotkey::on_key_down(app);
                    }
                    K_CG_EVENT_KEY_UP => {
                        crate::hotkey::on_key_up(app);
                    }
                    _ => {}
                }
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

pub fn paste_text() -> Result<(), String> {
    if !check_accessibility_permission() {
        return Err("Accessibility permission required".to_string());
    }

    unsafe {
        // Create event source
        let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE);
        if source.is_null() {
            return Err("Failed to create event source".to_string());
        }

        // Create Cmd+V key down event
        let key_down = CGEventCreateKeyboardEvent(source, K_VK_V, true);
        if key_down.is_null() {
            CFRelease(source);
            return Err("Failed to create key down event".to_string());
        }
        CGEventSetFlags(key_down, K_CG_EVENT_FLAG_MASK_COMMAND);

        // Create key up event
        let key_up = CGEventCreateKeyboardEvent(source, K_VK_V, false);
        if key_up.is_null() {
            CFRelease(key_down);
            CFRelease(source);
            return Err("Failed to create key up event".to_string());
        }
        CGEventSetFlags(key_up, K_CG_EVENT_FLAG_MASK_COMMAND);

        // Post events
        CGEventPost(K_CG_HID_EVENT_TAP, key_down);
        CGEventPost(K_CG_HID_EVENT_TAP, key_up);

        // Clean up
        CFRelease(key_up);
        CFRelease(key_down);
        CFRelease(source);
    }

    Ok(())
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

    let script = format!(
        r#"tell application "System Events"
            if not (exists login item "Fing") then
                make login item at end with properties {{path:"{}", hidden:false}}
            end if
        end tell"#,
        app_path
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
            let event_mask = K_CG_EVENT_MASK_FOR_KEY_DOWN | K_CG_EVENT_MASK_FOR_KEY_UP | K_CG_EVENT_MASK_FOR_FLAGS_CHANGED;

            let event_tap = CGEventTapCreate(
                K_CG_SESSION_EVENT_TAP,
                K_CG_HEAD_INSERT_EVENT_TAP,
                K_CG_EVENT_TAP_OPTION_LISTEN_ONLY,
                event_mask,
                event_tap_callback,
                std::ptr::null_mut(),
            );

            if event_tap.is_null() {
                tracing::error!("Failed to create event tap - accessibility permission may be required");
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
