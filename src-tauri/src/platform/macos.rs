// macOS-specific platform code

use std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
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
    fn CGEventKeyboardSetUnicodeString(
        event: *mut c_void,
        stringLength: std::ffi::c_ulong,
        unicodeString: *const u16,
    );
}

// CGEventTapLocation
const K_CG_HID_EVENT_TAP: i32 = 0;

// CGEventSourceStateID
const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM_STATE: i32 = 1;

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
