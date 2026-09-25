// Windows-specific platform code

use windows::core::{h, HSTRING};
use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Foundation::Uri;
use windows::Security::Authorization::AppCapabilityAccess::{
    AppCapability, AppCapabilityAccessStatus,
};
use windows::System::Launcher;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VK_RETURN, VK_TAB,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow};

// Windows does not require explicit accessibility permission for hotkeys

pub fn check_accessibility_permission() -> bool {
    // Windows doesn't have the same accessibility permission model as macOS
    true
}

pub fn request_accessibility_permission() -> bool {
    true
}

/// Microphone consent. Packaged (Store) builds run under Windows' per-app
/// privacy consent, which otherwise pops the OS dialog lazily on the first
/// capture activation (i.e. mid hotkey press). Unpackaged builds have no
/// consent gate at all.
pub fn check_microphone_permission() -> String {
    if !is_packaged() {
        return "granted".to_string();
    }

    match microphone_capability().and_then(|capability| capability.CheckAccess()) {
        Ok(status) => capability_status_to_string(status),
        Err(e) => {
            tracing::warn!("Microphone capability check failed: {}", e);
            "denied".to_string()
        }
    }
}

/// Prompt for microphone consent (joins until the dialog is answered, so
/// never call this on the main thread), or open the Windows privacy page when
/// the user already denied it: a denied capability cannot be re-prompted.
pub fn request_microphone_permission() {
    if !is_packaged() {
        return;
    }

    let capability = match microphone_capability() {
        Ok(capability) => capability,
        Err(e) => {
            tracing::warn!("Microphone capability lookup failed: {}", e);
            return;
        }
    };

    match capability.CheckAccess() {
        Ok(AppCapabilityAccessStatus::Allowed) => {}
        Ok(AppCapabilityAccessStatus::UserPromptRequired) => {
            match capability
                .RequestAccessAsync()
                .and_then(|operation| operation.join())
            {
                Ok(status) => tracing::info!(
                    "Microphone consent result: {}",
                    capability_status_to_string(status)
                ),
                Err(e) => tracing::warn!("Microphone consent request failed: {}", e),
            }
        }
        Ok(_) => open_microphone_privacy_settings(),
        Err(e) => tracing::warn!("Microphone capability check failed: {}", e),
    }
}

fn microphone_capability() -> windows::core::Result<AppCapability> {
    AppCapability::Create(h!("microphone"))
}

fn capability_status_to_string(status: AppCapabilityAccessStatus) -> String {
    match status {
        AppCapabilityAccessStatus::Allowed => "granted",
        AppCapabilityAccessStatus::UserPromptRequired => "prompt",
        AppCapabilityAccessStatus::DeniedByUser | AppCapabilityAccessStatus::DeniedBySystem => {
            "denied"
        }
        other => {
            tracing::warn!("Unexpected microphone capability status: {:?}", other);
            "denied"
        }
    }
    .to_string()
}

fn open_microphone_privacy_settings() {
    let result = Uri::CreateUri(h!("ms-settings:privacy-microphone"))
        .and_then(|uri| Launcher::LaunchUriAsync(&uri))
        .and_then(|operation| operation.join());
    if let Err(e) = result {
        tracing::warn!("Failed to open microphone privacy settings: {}", e);
    }
}

/// Filter text to printable characters only (security: prevent control char injection)
fn filter_printable(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Delay between characters when typing into Notepad. Windows 11 Notepad
/// resolves VK_PACKET characters late: when a whole batch arrives at once it
/// substitutes the most recently injected character for the pending ones
/// (repeated letters, trailing spaces). One character at a time avoids that.
const NOTEPAD_CHAR_DELAY_MS: u64 = 4;

/// Type text directly using SendInput with Unicode (no clipboard)
pub fn type_text(text: &str) -> Result<(), String> {
    let filtered = filter_printable(text);

    if filtered.is_empty() {
        return Ok(());
    }

    if let Some(notepad) = foreground_notepad() {
        tracing::debug!("Foreground window is Notepad, typing with per-character pacing");
        return type_text_paced(&filtered, notepad);
    }

    // Build input array: for each character, we need key down + key up
    let mut inputs: Vec<INPUT> = Vec::with_capacity(filtered.len() * 2);
    for c in filtered.chars() {
        push_char_inputs(&mut inputs, c);
    }

    send_inputs(&inputs)
}

/// Type one character per SendInput call, stopping if focus leaves the target
/// window so the remaining text never lands somewhere else.
fn type_text_paced(text: &str, hwnd: HWND) -> Result<(), String> {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(4);

    for (i, c) in text.chars().enumerate() {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(NOTEPAD_CHAR_DELAY_MS));
        }
        if unsafe { GetForegroundWindow() } != hwnd {
            return Err("Foreground window changed while typing".to_string());
        }

        inputs.clear();
        push_char_inputs(&mut inputs, c);
        send_inputs(&inputs)?;
    }

    Ok(())
}

/// The foreground window if it is Notepad (classic and Windows 11 Notepad
/// both register the top-level window class "Notepad").
fn foreground_notepad() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }

    let mut class = [0u16; 32];
    let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    if len <= 0 {
        return None;
    }

    "Notepad"
        .encode_utf16()
        .eq(class[..len as usize].iter().copied())
        .then_some(hwnd)
}

fn key_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Append key down + key up events for one character: newline and tab as
/// Enter/Tab keys, everything else as Unicode packets (one pair per UTF-16
/// unit, so surrogate pairs stay together).
fn push_char_inputs(inputs: &mut Vec<INPUT>, c: char) {
    let vk = match c {
        '\n' => VK_RETURN,
        '\t' => VK_TAB,
        _ => {
            let mut units = [0u16; 2];
            for &unit in c.encode_utf16(&mut units).iter() {
                inputs.push(key_input(0, unit, KEYEVENTF_UNICODE));
                inputs.push(key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
            return;
        }
    };

    inputs.push(key_input(vk, 0, 0));
    inputs.push(key_input(vk, 0, KEYEVENTF_KEYUP));
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn events(c: char) -> Vec<(u16, u16, KEYBD_EVENT_FLAGS)> {
        let mut inputs = Vec::new();
        push_char_inputs(&mut inputs, c);
        inputs
            .iter()
            .map(|input| {
                let ki = unsafe { input.Anonymous.ki };
                (ki.wVk, ki.wScan, ki.dwFlags)
            })
            .collect()
    }

    #[test]
    fn push_char_inputs_maps_newline_and_tab_to_keys() {
        assert_eq!(
            events('\n'),
            vec![(VK_RETURN, 0, 0), (VK_RETURN, 0, KEYEVENTF_KEYUP)]
        );
        assert_eq!(
            events('\t'),
            vec![(VK_TAB, 0, 0), (VK_TAB, 0, KEYEVENTF_KEYUP)]
        );
    }

    #[test]
    fn push_char_inputs_keeps_surrogate_pairs_together() {
        assert_eq!(
            events('a'),
            vec![
                (0, 'a' as u16, KEYEVENTF_UNICODE),
                (0, 'a' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
            ]
        );

        let units: Vec<u16> = events('😀').iter().map(|&(_, scan, _)| scan).collect();
        assert_eq!(units, vec![0xD83D, 0xD83D, 0xDE00, 0xDE00]);
    }
}
