// macOS-specific platform code
// Uses native APIs instead of AppleScript to avoid Automation permission

use enigo::{Enigo, Keyboard, Settings};
use std::ffi::c_char;
use std::ffi::c_int;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

type KernReturn = c_int;
type IoObject = u32;
type IoIterator = IoObject;
type IONotificationPortRef = *mut c_void;
type CFMutableDictionaryRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;

static KEYBOARD_DEVICE_OBSERVER_STARTED: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct CFDictionaryKeyCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
    hash: *const c_void,
}

#[repr(C)]
struct CFDictionaryValueCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: isize,
        key_callbacks: *const CFDictionaryKeyCallBacks,
        value_callbacks: *const CFDictionaryValueCallBacks,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);
    fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    static kCFBooleanTrue: *const c_void;
    static kCFRunLoopDefaultMode: CFStringRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryKeyCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryValueCallBacks;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IONotificationPortCreate(master_port: u32) -> IONotificationPortRef;
    fn IONotificationPortGetRunLoopSource(notify: IONotificationPortRef) -> CFRunLoopSourceRef;
    fn IOIteratorNext(iterator: IoIterator) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> KernReturn;
    fn IOServiceAddMatchingNotification(
        notify: IONotificationPortRef,
        notification_type: *const c_char,
        matching: CFMutableDictionaryRef,
        callback: unsafe extern "C" fn(*mut c_void, IoIterator),
        refcon: *mut c_void,
        notification: *mut IoIterator,
    ) -> KernReturn;
    fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

extern "C" {
    fn fing_microphone_authorization_status() -> u32;
    fn fing_request_microphone_access() -> bool;
}

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
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        if options.is_null() {
            tracing::warn!("Failed to create Accessibility prompt options");
            return check_accessibility_permission();
        }

        let trusted = AXIsProcessTrustedWithOptions(options);
        CFRelease(options);

        trusted
    }
}

/// Open System Preferences to the Microphone privacy pane
pub fn request_microphone_permission() {
    let status = check_microphone_permission();
    if status == "granted" {
        return;
    }

    if status == "prompt" {
        if unsafe { fing_request_microphone_access() } {
            return;
        }

        return;
    }

    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        .spawn();
}

struct KeyboardDeviceObserver {
    app: AppHandle,
}

pub fn start_keyboard_device_observer(app: AppHandle) {
    if KEYBOARD_DEVICE_OBSERVER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    std::thread::spawn(move || {
        if let Err(error) = run_keyboard_device_observer(app) {
            KEYBOARD_DEVICE_OBSERVER_STARTED.store(false, Ordering::SeqCst);
            tracing::warn!("Keyboard device observer stopped: {}", error);
        }
    });
}

fn run_keyboard_device_observer(app: AppHandle) -> Result<(), String> {
    unsafe {
        let notification_port = IONotificationPortCreate(0);
        if notification_port.is_null() {
            return Err("failed to create IOKit notification port".to_string());
        }

        let run_loop_source = IONotificationPortGetRunLoopSource(notification_port);
        if run_loop_source.is_null() {
            return Err("failed to create IOKit notification run loop source".to_string());
        }

        CFRunLoopAddSource(
            CFRunLoopGetCurrent(),
            run_loop_source,
            kCFRunLoopDefaultMode,
        );

        let observer = Box::into_raw(Box::new(KeyboardDeviceObserver { app }));
        register_keyboard_notification(
            notification_port,
            c"IOServiceFirstMatch".as_ptr(),
            observer,
        )?;
        register_keyboard_notification(
            notification_port,
            c"IOServiceTerminate".as_ptr(),
            observer,
        )?;

        tracing::info!("Keyboard device observer started");
        CFRunLoopRun();
    }

    Ok(())
}

unsafe fn register_keyboard_notification(
    notification_port: IONotificationPortRef,
    notification_type: *const c_char,
    observer: *mut KeyboardDeviceObserver,
) -> Result<(), String> {
    let matching = IOServiceMatching(c"IOHIDKeyboard".as_ptr());
    if matching.is_null() {
        return Err("failed to create IOHIDKeyboard matching dictionary".to_string());
    }

    let mut iterator: IoIterator = 0;
    let status = IOServiceAddMatchingNotification(
        notification_port,
        notification_type,
        matching,
        keyboard_device_changed,
        observer.cast::<c_void>(),
        &mut iterator,
    );
    if status != 0 {
        return Err(format!(
            "failed to register keyboard notification ({status})"
        ));
    }

    // Arm the notification without treating already-connected keyboards as a
    // hotkey recovery event.
    drain_io_iterator(iterator);
    Ok(())
}

unsafe extern "C" fn keyboard_device_changed(refcon: *mut c_void, iterator: IoIterator) {
    if !drain_io_iterator(iterator) || refcon.is_null() {
        return;
    }

    let observer = &*(refcon as *const KeyboardDeviceObserver);
    crate::hotkey_listener::restart_hotkey_listener(
        observer.app.clone(),
        crate::hotkey_listener::HotkeyRestartReason::KeyboardDeviceChanged,
    );
}

unsafe fn drain_io_iterator(iterator: IoIterator) -> bool {
    let mut saw_object = false;

    loop {
        let object = IOIteratorNext(iterator);
        if object == 0 {
            break;
        }

        saw_object = true;
        let _ = IOObjectRelease(object);
    }

    saw_object
}

/// Check microphone permission without opening the audio device.
pub fn check_microphone_permission() -> String {
    let status = unsafe { fing_microphone_authorization_status() };

    match status {
        0 => "prompt".to_string(),
        3 => "granted".to_string(),
        1 | 2 => "denied".to_string(),
        _ => {
            tracing::warn!("Unknown microphone authorization status: {}", status);
            "denied".to_string()
        }
    }
}

pub fn activate_current_app() {
    unsafe {
        let app_class = objc_getClass(c"NSApplication".as_ptr());
        if app_class.is_null() {
            tracing::warn!("Failed to get NSApplication class");
            return;
        }

        let shared_sel = sel_registerName(c"sharedApplication".as_ptr());
        let app = objc_msgSend(app_class, shared_sel);
        if app.is_null() {
            tracing::warn!("Failed to get shared NSApplication");
            return;
        }

        let activate_sel = sel_registerName(c"activateIgnoringOtherApps:".as_ptr());
        let _: *mut c_void = objc_msgSend(app, activate_sel, 1 as c_int);
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
