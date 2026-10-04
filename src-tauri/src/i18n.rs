use crate::settings::UiLanguage;
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayTranslations {
    pub complete_setup: String,
    pub quit: String,
    pub open_app: String,
    pub history: String,
    pub settings: String,
    pub update_available: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndicatorTranslations {
    pub recording_limit_reached: String,
    pub microphone_unavailable: String,
    pub model_load_failed: String,
    pub transcription_failed: String,
}

#[derive(Debug, Deserialize)]
pub struct NativeTranslations {
    pub tray: TrayTranslations,
    pub indicator: IndicatorTranslations,
}

static EN: LazyLock<NativeTranslations> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../locales/en.json"))
        .expect("English native translations must be valid")
});
static DE: LazyLock<NativeTranslations> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../locales/de.json"))
        .expect("German native translations must be valid")
});

pub fn for_language(language: UiLanguage) -> &'static NativeTranslations {
    match language {
        UiLanguage::En => &EN,
        UiLanguage::De => &DE,
    }
}

pub fn current() -> &'static NativeTranslations {
    for_language(crate::settings::load_settings_sync().ui_language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_parse_and_have_required_values() {
        for catalog in [&*EN, &*DE] {
            assert!(!catalog.tray.complete_setup.is_empty());
            assert!(!catalog.tray.quit.is_empty());
            assert!(!catalog.tray.open_app.is_empty());
            assert!(!catalog.tray.history.is_empty());
            assert!(!catalog.tray.settings.is_empty());
            assert!(!catalog.tray.update_available.is_empty());
        }
    }

    #[test]
    fn indicator_messages_are_short_single_lines() {
        for catalog in [&*EN, &*DE] {
            let indicator = &catalog.indicator;
            for message in [
                &indicator.recording_limit_reached,
                &indicator.microphone_unavailable,
                &indicator.model_load_failed,
                &indicator.transcription_failed,
            ] {
                assert!(!message.is_empty());
                assert!(message.chars().count() <= 32, "too long: {message}");
                assert!(!message.contains('\n') && !message.contains('{'));
            }
        }
    }
}
