// Windows-specific platform code

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use tauri::AppHandle;

#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
            HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
        },
        Threading::GetCurrentThreadId,
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
            KEYEVENTF_UNICODE, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
            HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
            WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

// Windows Virtual Key codes (platform-independent constants)
const VK_F1: u16 = 0x70;
const VK_F2: u16 = 0x71;
const VK_F3: u16 = 0x72;
const VK_F4: u16 = 0x73;
const VK_F5: u16 = 0x74;
const VK_F6: u16 = 0x75;
const VK_F7: u16 = 0x76;
const VK_F8: u16 = 0x77;
const VK_F9: u16 = 0x78;
const VK_F10: u16 = 0x79;
const VK_F11: u16 = 0x7A;
const VK_F12: u16 = 0x7B;
const VK_F13: u16 = 0x7C;
const VK_F14: u16 = 0x7D;
const VK_F15: u16 = 0x7E;
const VK_SPACE: u16 = 0x20;

#[cfg(target_os = "windows")]
const VK_CONTROL_WIN: VIRTUAL_KEY = 0x11;
#[cfg(target_os = "windows")]
const VK_SHIFT_WIN: VIRTUAL_KEY = 0x10;
#[cfg(target_os = "windows")]
const VK_LSHIFT_WIN: VIRTUAL_KEY = 0xA0;
#[cfg(target_os = "windows")]
const VK_RSHIFT_WIN: VIRTUAL_KEY = 0xA1;
#[cfg(target_os = "windows")]
const VK_MENU_WIN: VIRTUAL_KEY = 0x12;
#[cfg(target_os = "windows")]
const VK_LMENU_WIN: VIRTUAL_KEY = 0xA4;
#[cfg(target_os = "windows")]
const VK_RMENU_WIN: VIRTUAL_KEY = 0xA5;

// Global app handle storage
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

// Track if hotkey is currently pressed (prevents auto-repeat)
static HOTKEY_ACTIVE: AtomicBool = AtomicBool::new(false);

// Hook thread ID for posting quit message
#[cfg(target_os = "windows")]
static HOOK_THREAD_ID: OnceLock<u32> = OnceLock::new();

/// Parsed hotkey configuration
#[derive(Clone)]
struct ParsedHotkey {
    vk_code: u16,
    requires_ctrl: bool,
    requires_shift: bool,
    requires_alt: bool,
}

impl Default for ParsedHotkey {
    fn default() -> Self {
        Self {
            vk_code: VK_F8,
            requires_ctrl: false,
            requires_shift: false,
            requires_alt: false,
        }
    }
}

// Current hotkey configuration
static CURRENT_HOTKEY: OnceLock<RwLock<ParsedHotkey>> = OnceLock::new();

fn get_hotkey_config() -> &'static RwLock<ParsedHotkey> {
    CURRENT_HOTKEY.get_or_init(|| RwLock::new(ParsedHotkey::default()))
}

#[cfg(target_os = "windows")]
fn get_current_hotkey() -> ParsedHotkey {
    get_hotkey_config()
        .read()
        .map(|h| h.clone())
        .unwrap_or_default()
}

/// Map a key name to Windows virtual key code
fn key_name_to_vk(key: &str) -> Option<u16> {
    match key.to_uppercase().as_str() {
        "F1" => Some(VK_F1),
        "F2" => Some(VK_F2),
        "F3" => Some(VK_F3),
        "F4" => Some(VK_F4),
        "F5" => Some(VK_F5),
        "F6" => Some(VK_F6),
        "F7" => Some(VK_F7),
        "F8" => Some(VK_F8),
        "F9" => Some(VK_F9),
        "F10" => Some(VK_F10),
        "F11" => Some(VK_F11),
        "F12" => Some(VK_F12),
        "F13" => Some(VK_F13),
        "F14" => Some(VK_F14),
        "F15" => Some(VK_F15),
        "A" => Some(0x41),
        "B" => Some(0x42),
        "C" => Some(0x43),
        "D" => Some(0x44),
        "E" => Some(0x45),
        "F" => Some(0x46),
        "G" => Some(0x47),
        "H" => Some(0x48),
        "I" => Some(0x49),
        "J" => Some(0x4A),
        "K" => Some(0x4B),
        "L" => Some(0x4C),
        "M" => Some(0x4D),
        "N" => Some(0x4E),
        "O" => Some(0x4F),
        "P" => Some(0x50),
        "Q" => Some(0x51),
        "R" => Some(0x52),
        "S" => Some(0x53),
        "T" => Some(0x54),
        "U" => Some(0x55),
        "V" => Some(0x56),
        "W" => Some(0x57),
        "X" => Some(0x58),
        "Y" => Some(0x59),
        "Z" => Some(0x5A),
        "0" => Some(0x30),
        "1" => Some(0x31),
        "2" => Some(0x32),
        "3" => Some(0x33),
        "4" => Some(0x34),
        "5" => Some(0x35),
        "6" => Some(0x36),
        "7" => Some(0x37),
        "8" => Some(0x38),
        "9" => Some(0x39),
        "SPACE" => Some(VK_SPACE),
        _ => None,
    }
}

/// Check if a modifier key is currently pressed
#[cfg(target_os = "windows")]
fn is_modifier_pressed(vk: VIRTUAL_KEY) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) & 0x8000u16 as i16) != 0 }
}

/// Maximum allowed hotkey string length
const MAX_HOTKEY_LENGTH: usize = 50;

/// Maximum number of parts in a hotkey combination
const MAX_HOTKEY_PARTS: usize = 5;

/// Update the current hotkey (supports combinations like "Ctrl+Space")
pub fn set_hotkey(key: &str) -> Result<(), String> {
    // Validate total length
    if key.len() > MAX_HOTKEY_LENGTH {
        return Err(format!("Hotkey too long (max {} chars)", MAX_HOTKEY_LENGTH));
    }

    // Validate characters
    if !key
        .chars()
        .all(|c| c.is_alphanumeric() || c == '+' || c == ' ')
    {
        return Err("Hotkey contains invalid characters".to_string());
    }

    let parts: Vec<&str> = key.split('+').collect();

    if parts.len() > MAX_HOTKEY_PARTS {
        return Err(format!(
            "Too many keys in combination (max {})",
            MAX_HOTKEY_PARTS
        ));
    }

    let mut requires_ctrl = false;
    let mut requires_shift = false;
    let mut requires_alt = false;
    let mut vk_code: Option<u16> = None;

    for part in parts {
        let part = part.trim();

        if part.len() > 10 {
            return Err(format!("Invalid key name: {}", part));
        }

        match part.to_uppercase().as_str() {
            "CTRL" | "CONTROL" => requires_ctrl = true,
            "SHIFT" => requires_shift = true,
            "ALT" | "OPTION" => requires_alt = true,
            _ => {
                if vk_code.is_some() {
                    return Err(format!("Multiple base keys in hotkey: {}", key));
                }
                vk_code = Some(
                    key_name_to_vk(part).ok_or_else(|| format!("Unknown key: {}", part))?,
                );
            }
        }
    }

    let vk_code = vk_code.ok_or("No base key in hotkey")?;

    let config = ParsedHotkey {
        vk_code,
        requires_ctrl,
        requires_shift,
        requires_alt,
    };

    let hotkey_lock = get_hotkey_config();
    let mut hotkey = hotkey_lock
        .write()
        .map_err(|e| format!("Lock error: {}", e))?;
    *hotkey = config;

    HOTKEY_ACTIVE.store(false, Ordering::SeqCst);

    tracing::info!(
        "Hotkey updated to: {} (vk: 0x{:02X}, ctrl: {}, shift: {}, alt: {})",
        key,
        vk_code,
        requires_ctrl,
        requires_shift,
        requires_alt
    );
    Ok(())
}

/// Low-level keyboard hook callback
#[cfg(target_os = "windows")]
unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let kb_struct = *(lparam as *const KBDLLHOOKSTRUCT);

        // Ignore injected events (from SendInput) to prevent loops
        if (kb_struct.flags & LLKHF_INJECTED) != 0 {
            return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
        }

        let config = get_current_hotkey();
        let vk = kb_struct.vkCode as u16;

        // Check if it's our target key
        if vk == config.vk_code {
            // Check modifiers
            let ctrl_ok = !config.requires_ctrl || is_modifier_pressed(VK_CONTROL_WIN);
            let shift_ok = !config.requires_shift
                || is_modifier_pressed(VK_SHIFT_WIN)
                || is_modifier_pressed(VK_LSHIFT_WIN)
                || is_modifier_pressed(VK_RSHIFT_WIN);
            let alt_ok = !config.requires_alt
                || is_modifier_pressed(VK_MENU_WIN)
                || is_modifier_pressed(VK_LMENU_WIN)
                || is_modifier_pressed(VK_RMENU_WIN);

            if ctrl_ok && shift_ok && alt_ok {
                let was_active = HOTKEY_ACTIVE.load(Ordering::SeqCst);

                match wparam as u32 {
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        if !was_active {
                            HOTKEY_ACTIVE.store(true, Ordering::SeqCst);
                            if let Some(app) = APP_HANDLE.get() {
                                crate::hotkey::on_key_down(app);
                            }
                        }
                    }
                    WM_KEYUP | WM_SYSKEYUP => {
                        if was_active {
                            HOTKEY_ACTIVE.store(false, Ordering::SeqCst);
                            if let Some(app) = APP_HANDLE.get() {
                                crate::hotkey::on_key_up(app);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// Register global hotkey using low-level keyboard hook
#[cfg(target_os = "windows")]
pub fn register_global_hotkey(app: AppHandle) -> Result<(), String> {
    // Store app handle globally
    APP_HANDLE
        .set(app)
        .map_err(|_| "App handle already set".to_string())?;

    // Spawn background thread for keyboard hook message loop
    std::thread::spawn(|| {
        unsafe {
            // Store thread ID for cleanup
            let thread_id = GetCurrentThreadId();
            HOOK_THREAD_ID.set(thread_id).ok();

            // Install low-level keyboard hook
            let hook: HHOOK = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(low_level_keyboard_proc),
                GetModuleHandleW(std::ptr::null()),
                0,
            );

            if hook.is_null() {
                tracing::error!("Failed to install keyboard hook");
                return;
            }

            tracing::info!("Global hotkey (F8) registered via low-level keyboard hook");

            // Message loop - required for low-level hooks to work
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.message == WM_QUIT {
                    break;
                }
                DispatchMessageW(&msg);
            }

            // Cleanup
            UnhookWindowsHookEx(hook);
            tracing::info!("Keyboard hook uninstalled");
        }
    });

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn register_global_hotkey(_app: AppHandle) -> Result<(), String> {
    Ok(())
}

pub fn check_accessibility_permission() -> bool {
    // Windows doesn't have the same accessibility permission model as macOS
    true
}

pub fn request_accessibility_permission() -> bool {
    true
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
    let exe_path =
        std::env::current_exe().map_err(|e| format!("Failed to get exe path: {}", e))?;
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
