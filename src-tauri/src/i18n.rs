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
    pub check_for_updates: String,
    pub update_available: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationTranslations {
    pub recording_stopped_title: String,
    pub maximum_recording_duration: String,
    pub microphone_error_title: String,
    pub microphone_start_failed: String,
    pub model_error_title: String,
    pub model_load_failed: String,
    pub transcription_error_title: String,
    pub transcription_failed: String,
}

#[derive(Debug, Deserialize)]
pub struct NativeTranslations {
    pub tray: TrayTranslations,
    pub notifications: NotificationTranslations,
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

pub fn interpolate(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_string(), |message, (key, value)| {
            message.replace(&format!("{{{key}}}"), value)
        })
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
            assert!(!catalog.tray.check_for_updates.is_empty());
            assert!(!catalog.tray.update_available.is_empty());
            assert!(!catalog.notifications.recording_stopped_title.is_empty());
            assert!(!catalog.notifications.maximum_recording_duration.is_empty());
            assert!(!catalog.notifications.microphone_error_title.is_empty());
            assert!(catalog
                .notifications
                .microphone_start_failed
                .contains("{error}"));
            assert!(!catalog.notifications.model_error_title.is_empty());
            assert!(catalog.notifications.model_load_failed.contains("{error}"));
            assert!(!catalog.notifications.transcription_error_title.is_empty());
            assert!(catalog
                .notifications
                .transcription_failed
                .contains("{error}"));
        }
    }

    #[test]
    fn interpolation_replaces_named_values() {
        assert_eq!(
            interpolate("Failed: {error}", &[("error", "offline")]),
            "Failed: offline"
        );
    }

    #[test]
    fn tray_labels_follow_language() {
        assert_eq!(for_language(UiLanguage::En).tray.settings, "Settings");
        assert_eq!(for_language(UiLanguage::De).tray.settings, "Einstellungen");
        assert_eq!(
            for_language(UiLanguage::De).tray.update_available,
            "Update verfügbar"
        );
    }
}
