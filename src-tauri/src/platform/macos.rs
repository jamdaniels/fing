// macOS-specific platform code
// Uses native APIs instead of AppleScript to avoid Automation permission

use enigo::{Enigo, Keyboard, Settings};
use smappservice_rs::{AppService, ServiceStatus, ServiceType};
use std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

// Objective-C runtime bindings
#[link(name = "objc", kind = "dylib")]
extern "C" {
    fn objc_getClass(name: *const i8) -> *mut c_void;
    fn sel_registerName(name: *const i8) -> *mut c_void;
    fn objc_msgSend(obj: *mut c_void, sel: *mut c_void, ...) -> *mut c_void;
}

pub fn check_accessibility_permission() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn request_accessibility_permission() -> bool {
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

    let mut capture = AudioCapture::new();
    match capture.test_microphone() {
        Ok(test) => {
            if test.is_receiving_audio || test.peak_level > 0.0 {
                "granted".to_string()
            } else {
                let devices = AudioCapture::list_devices();
                if devices.is_empty() {
                    "denied".to_string()
                } else {
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

/// Type text using enigo (Accessibility permission only, no Automation)
pub fn type_text(text: &str) -> Result<(), String> {
    if !check_accessibility_permission() {
        return Err("Accessibility permission required".to_string());
    }

    let filtered = filter_printable(text);
    if filtered.is_empty() {
        return Ok(());
    }

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init enigo: {e:?}"))?;

    enigo
        .text(&filtered)
        .map_err(|e| format!("Failed to type text: {e:?}"))?;

    Ok(())
}

/// Enable auto-start on login using SMAppService (no Automation permission)
pub fn enable_auto_start() -> Result<(), String> {
    let service = AppService::new(ServiceType::MainApp);
    service
        .register()
        .map_err(|e| format!("Failed to enable auto-start: {e:?}"))?;

    tracing::info!("Auto-start enabled via SMAppService");
    Ok(())
}

/// Disable auto-start on login using SMAppService
pub fn disable_auto_start() -> Result<(), String> {
    let service = AppService::new(ServiceType::MainApp);
    service
        .unregister()
        .map_err(|e| format!("Failed to disable auto-start: {e:?}"))?;

    tracing::info!("Auto-start disabled");
    Ok(())
}

/// Check if auto-start is enabled using SMAppService
pub fn is_auto_start_enabled() -> bool {
    let service = AppService::new(ServiceType::MainApp);
    service.status() == ServiceStatus::Enabled
}

/// Get the bundle identifier of the frontmost application using NSWorkspace
pub fn get_frontmost_app() -> Option<String> {
    unsafe {
        // Get NSWorkspace class
        let workspace_class = objc_getClass(c"NSWorkspace".as_ptr());
        if workspace_class.is_null() {
            tracing::warn!("Failed to get NSWorkspace class");
            return None;
        }

        // Get shared workspace: [NSWorkspace sharedWorkspace]
        let shared_sel = sel_registerName(c"sharedWorkspace".as_ptr());
        let workspace = objc_msgSend(workspace_class, shared_sel);
        if workspace.is_null() {
            tracing::warn!("Failed to get shared workspace");
            return None;
        }

        // Get frontmost application: [workspace frontmostApplication]
        let frontmost_sel = sel_registerName(c"frontmostApplication".as_ptr());
        let app = objc_msgSend(workspace, frontmost_sel);
        if app.is_null() {
            tracing::warn!("No frontmost application");
            return None;
        }

        // Get bundle identifier: [app bundleIdentifier]
        let bundle_id_sel = sel_registerName(c"bundleIdentifier".as_ptr());
        let bundle_id_nsstring = objc_msgSend(app, bundle_id_sel);
        if bundle_id_nsstring.is_null() {
            tracing::warn!("No bundle identifier");
            return None;
        }

        // Convert NSString to Rust String
        let utf8_sel = sel_registerName(c"UTF8String".as_ptr());
        let utf8_ptr = objc_msgSend(bundle_id_nsstring, utf8_sel) as *const i8;
        if utf8_ptr.is_null() {
            return None;
        }

        let c_str = std::ffi::CStr::from_ptr(utf8_ptr);
        let bundle_id = c_str.to_string_lossy().to_string();

        tracing::debug!("Captured frontmost app: {}", bundle_id);
        Some(bundle_id)
    }
}

/// Activate an application by bundle identifier using `open -b`
pub fn activate_app(bundle_id: &str) -> Result<(), String> {
    let output = std::process::Command::new("open")
        .arg("-b")
        .arg(bundle_id)
        .output()
        .map_err(|e| format!("Failed to run open command: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to activate app {bundle_id}: {stderr}"));
    }

    tracing::debug!("Activated app: {}", bundle_id);
    Ok(())
}
