// Direct text input (no clipboard)

use crate::platform;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PasteResult {
    Success,
    Failed(String),
    NoAccessibility,
}

impl PasteResult {
    pub fn should_notify(&self) -> bool {
        matches!(self, PasteResult::Failed(_) | PasteResult::NoAccessibility)
    }
}

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
