// Windows-specific platform code (stub)

/// Enable auto-start on login (stub for Windows)
pub fn enable_auto_start() -> Result<(), String> {
    // TODO: Implement using Windows Registry
    // HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
    tracing::warn!("Windows auto-start not yet implemented");
    Ok(())
}

/// Disable auto-start on login (stub for Windows)
pub fn disable_auto_start() -> Result<(), String> {
    // TODO: Implement using Windows Registry
    tracing::warn!("Windows auto-start not yet implemented");
    Ok(())
}

/// Check if auto-start is enabled (stub for Windows)
pub fn is_auto_start_enabled() -> bool {
    // TODO: Implement using Windows Registry
    false
}

pub fn check_accessibility_permission() -> bool {
    // Windows doesn't have the same accessibility permission model as macOS
    true
}

pub fn request_accessibility_permission() -> bool {
    true
}

/// Type text directly (no clipboard) - stub for Windows
pub fn type_text(_text: &str) -> Result<(), String> {
    // TODO: Implement using SendInput with KEYEVENTF_UNICODE
    Err("Direct text input not yet implemented on Windows".to_string())
}

/// Set the hotkey (stub for Windows)
pub fn set_hotkey(_key: &str) -> Result<(), String> {
    // TODO: Implement Windows hotkey registration
    tracing::warn!("Windows hotkey change not yet implemented");
    Ok(())
}
