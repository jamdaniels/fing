// Windows-specific platform code

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
        HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
    },
    UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    },
};

// Windows does not require explicit accessibility permission for hotkeys

pub fn check_accessibility_permission() -> bool {
    // Windows doesn't have the same accessibility permission model as macOS
    true
}

pub fn request_accessibility_permission() -> bool {
    true
}

/// Check microphone permission - Windows doesn't require explicit permission
/// but we still check if audio capture works
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
                    // On Windows, if we have devices but no audio, it's likely just silence
                    "granted".to_string()
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

/// Type text directly using SendInput with Unicode (no clipboard)
#[cfg(target_os = "windows")]
pub fn type_text(text: &str) -> Result<(), String> {
    let filtered = filter_printable(text);
    let utf16: Vec<u16> = filtered.encode_utf16().collect();

    if utf16.is_empty() {
        return Ok(());
    }

    // Build input array: for each character, we need key down + key up
    let mut inputs: Vec<INPUT> = Vec::with_capacity(utf16.len() * 2);

    for &ch in &utf16 {
        // Handle newline as Enter key
        if ch == 0x000A {
            // Key down
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0x0D, // VK_RETURN
                        wScan: 0,
                        dwFlags: 0,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            // Key up
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0x0D,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            continue;
        }

        // Handle tab as Tab key
        if ch == 0x0009 {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0x09, // VK_TAB
                        wScan: 0,
                        dwFlags: 0,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: 0x09,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            });
            continue;
        }

        // Unicode character - key down
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        // Unicode character - key up
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    if inputs.is_empty() {
        return Ok(());
    }

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };

    if sent != inputs.len() as u32 {
        return Err(format!(
            "SendInput sent {} of {} inputs",
            sent,
            inputs.len()
        ));
    }

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn type_text(_text: &str) -> Result<(), String> {
    Err("type_text only implemented on Windows".to_string())
}

// Registry path for auto-start
#[cfg(target_os = "windows")]
const AUTO_START_REG_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(target_os = "windows")]
const AUTO_START_VALUE_NAME: &str = "Fing";

/// Convert a Rust string to a null-terminated wide string
#[cfg(target_os = "windows")]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Enable auto-start on login using Windows Registry
#[cfg(target_os = "windows")]
pub fn enable_auto_start() -> Result<(), String> {
    let exe_path = std::env::current_exe().map_err(|e| format!("Failed to get exe path: {}", e))?;
    let exe_path_str = exe_path.to_string_lossy().to_string();
    let exe_path_wide = to_wide_null(&exe_path_str);

    let reg_path_wide = to_wide_null(AUTO_START_REG_PATH);
    let value_name_wide = to_wide_null(AUTO_START_VALUE_NAME);

    unsafe {
        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            reg_path_wide.as_ptr(),
            0,
            KEY_WRITE,
            &mut hkey,
        );

        if result != 0 {
            return Err(format!("Failed to open registry key: error {}", result));
        }

        let result = RegSetValueExW(
            hkey,
            value_name_wide.as_ptr(),
            0,
            REG_SZ,
            exe_path_wide.as_ptr() as *const u8,
            (exe_path_wide.len() * 2) as u32,
        );

        RegCloseKey(hkey);

        if result != 0 {
            return Err(format!("Failed to set registry value: error {}", result));
        }
    }

    tracing::info!("Auto-start enabled for: {}", exe_path_str);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn enable_auto_start() -> Result<(), String> {
    tracing::warn!("Windows auto-start not available on this platform");
    Ok(())
}

/// Disable auto-start on login
#[cfg(target_os = "windows")]
pub fn disable_auto_start() -> Result<(), String> {
    let reg_path_wide = to_wide_null(AUTO_START_REG_PATH);
    let value_name_wide = to_wide_null(AUTO_START_VALUE_NAME);

    unsafe {
        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            reg_path_wide.as_ptr(),
            0,
            KEY_WRITE,
            &mut hkey,
        );

        if result != 0 {
            // Key doesn't exist or can't open - that's fine, nothing to delete
            return Ok(());
        }

        let result = RegDeleteValueW(hkey, value_name_wide.as_ptr());
        RegCloseKey(hkey);

        // Error code 2 means value doesn't exist - that's fine
        if result != 0 && result != 2 {
            return Err(format!("Failed to delete registry value: error {}", result));
        }
    }

    tracing::info!("Auto-start disabled");
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn disable_auto_start() -> Result<(), String> {
    tracing::warn!("Windows auto-start not available on this platform");
    Ok(())
}

/// Check if auto-start is enabled
#[cfg(target_os = "windows")]
pub fn is_auto_start_enabled() -> bool {
    let reg_path_wide = to_wide_null(AUTO_START_REG_PATH);
    let value_name_wide = to_wide_null(AUTO_START_VALUE_NAME);

    unsafe {
        let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            reg_path_wide.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        );

        if result != 0 {
            return false;
        }

        // Query the value to see if it exists
        let result = RegQueryValueExW(
            hkey,
            value_name_wide.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );

        RegCloseKey(hkey);
        result == 0
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_auto_start_enabled() -> bool {
    false
}
