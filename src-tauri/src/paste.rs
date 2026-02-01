// Direct text input (no clipboard)

use crate::platform;

/// Result of a paste/type operation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PasteResult {
    /// Text was successfully typed into the active application.
    Success,
    /// Platform-specific typing failed.
    Failed(String),
    /// macOS accessibility permission not granted.
    NoAccessibility,
}

impl PasteResult {
    pub fn should_notify(&self) -> bool {
        matches!(self, PasteResult::Failed(_) | PasteResult::NoAccessibility)
    }
}

/// Type text directly into the active application (no clipboard).
///
/// Uses platform-specific APIs (CGEventPost on macOS, SendInput on Windows).
pub fn paste_text(text: &str) -> PasteResult {
    #[cfg(target_os = "macos")]
    {
        if !platform::check_accessibility_permission() {
            return PasteResult::NoAccessibility;
        }
    }

    match platform::type_text(text) {
        Ok(()) => PasteResult::Success,
        Err(e) => {
            tracing::warn!("Direct text input failed: {}", e);
            PasteResult::Failed(e)
        }
    }
}
