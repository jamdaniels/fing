// Windows-specific platform code

use windows::core::HSTRING;
use windows::ApplicationModel::{StartupTask, StartupTaskState};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
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

/// Whether the process runs with MSIX package identity (installed from the
/// Microsoft Store). Unpackaged (NSIS) installs have no package identity.
pub fn is_packaged() -> bool {
    use windows_sys::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    let mut length: u32 = 0;
    let result = unsafe { GetCurrentPackageFullName(&mut length, std::ptr::null_mut()) };
    result != APPMODEL_ERROR_NO_PACKAGE
}

/// Task id declared in the MSIX manifest (`windows.startupTask` extension).
const STARTUP_TASK_ID: &str = "FingStartup";

async fn startup_task() -> Result<StartupTask, String> {
    StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK_ID))
        .map_err(|e| format!("StartupTask lookup failed: {e}"))?
        .await
        .map_err(|e| format!("StartupTask lookup failed: {e}"))
}

fn is_enabled_state(state: StartupTaskState) -> bool {
    state == StartupTaskState::Enabled || state == StartupTaskState::EnabledByPolicy
}

/// Autostart for the packaged (Store) build. The registry Run key used by
/// tauri-plugin-autostart is virtualized inside an MSIX container and the
/// install path changes on every Store update, so the StartupTask API is
/// the only mechanism that works there.
pub async fn set_startup_task_enabled(enabled: bool) -> Result<(), String> {
    let task = startup_task().await?;

    if !enabled {
        return task
            .Disable()
            .map_err(|e| format!("StartupTask disable failed: {e}"));
    }

    let state = task
        .RequestEnableAsync()
        .map_err(|e| format!("StartupTask enable failed: {e}"))?
        .await
        .map_err(|e| format!("StartupTask enable failed: {e}"))?;

    if is_enabled_state(state) {
        Ok(())
    } else if state == StartupTaskState::DisabledByUser {
        Err(
            "Startup was disabled in Windows Settings > Apps > Startup and must be re-enabled there"
                .to_string(),
        )
    } else if state == StartupTaskState::DisabledByPolicy {
        Err("Startup is disabled by system policy".to_string())
    } else {
        Err("Startup could not be enabled".to_string())
    }
}

pub async fn is_startup_task_enabled() -> Result<bool, String> {
    let state = startup_task()
        .await?
        .State()
        .map_err(|e| format!("StartupTask state failed: {e}"))?;
    Ok(is_enabled_state(state))
}
