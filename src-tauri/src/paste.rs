// Clipboard and paste functionality

use arboard::Clipboard;
use serde::Serialize;

use crate::platform;

#[derive(Debug, Clone, Serialize)]
pub enum PasteResult {
    Success,
    ClipboardOnlyElevated,
    ClipboardOnlySent,
}

impl PasteResult {
    pub fn should_notify(&self) -> bool {
        !matches!(self, PasteResult::Success)
    }
}

pub fn set_clipboard_and_paste(text: &str) -> PasteResult {
    // Set clipboard
    let mut clipboard = match Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to access clipboard: {}", e);
            return PasteResult::ClipboardOnlySent;
        }
    };

    if let Err(e) = clipboard.set_text(text) {
        tracing::error!("Failed to set clipboard text: {}", e);
        return PasteResult::ClipboardOnlySent;
    }

    // Check accessibility permission on macOS
    #[cfg(target_os = "macos")]
    {
        if !platform::check_accessibility_permission() {
            tracing::warn!("Accessibility permission not granted");
            return PasteResult::ClipboardOnlyElevated;
        }
    }

    // Attempt paste
    match platform::paste_text() {
        Ok(()) => PasteResult::Success,
        Err(e) => {
            tracing::warn!("Paste failed: {}", e);
            PasteResult::ClipboardOnlySent
        }
    }
}
