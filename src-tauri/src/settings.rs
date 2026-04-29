use crate::model::ModelVariant;
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, RwLock};
use tokio::fs;
use tokio::sync::Mutex;

/// User-selected theme preference.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// History retention mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub enum HistoryMode {
    #[serde(rename = "off")]
    Off,
    #[default]
    #[serde(rename = "30d")]
    ThirtyDays,
}

/// Cached settings to reduce disk I/O
static SETTINGS_CACHE: RwLock<Option<Settings>> = RwLock::new(None);
static SETTINGS_UPDATE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// User-configurable application settings (persisted to JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub hotkey: String,
    pub model_path: String,
    pub selected_microphone_id: Option<String>,
    pub auto_start: bool,
    pub sound_enabled: bool,
    pub paste_enabled: bool,
    #[serde(default)]
    pub history_mode: HistoryMode,
    #[serde(default)]
    pub onboarding_completed: bool,
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
    #[serde(default)]
    pub active_model_variant: ModelVariant,
    pub theme: Theme,
    #[serde(default)]
    pub lazy_model_loading: bool,
    #[serde(default)]
    pub dictionary_terms: Vec<String>,
}

fn default_languages() -> Vec<String> {
    vec!["en".to_string()]
}

fn merge_settings_update(latest: Settings, incoming: Settings) -> Settings {
    let incoming_completed = incoming.onboarding_completed;
    let mut merged = sanitize_settings(incoming);
    merged.onboarding_completed = latest.onboarding_completed || incoming_completed;
    merged
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "F9".to_string(),
            model_path: String::new(),
            selected_microphone_id: None,
            auto_start: false,
            sound_enabled: true,
            paste_enabled: true,
            history_mode: HistoryMode::default(),
            onboarding_completed: false,
            languages: default_languages(),
            active_model_variant: ModelVariant::default(),
            theme: Theme::default(),
            lazy_model_loading: false,
            dictionary_terms: Vec::new(),
        }
    }
}

/// Load settings from cache or disk (async).
pub async fn load_settings() -> Settings {
    // Check cache first
    if let Ok(cache) = SETTINGS_CACHE.read() {
        if let Some(ref settings) = *cache {
            return settings.clone();
        }
    }

    load_settings_uncached().await
}

pub async fn load_settings_uncached() -> Settings {
    let settings = load_settings_from_disk().await;
    // Update cache
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = Some(settings.clone());
    }

    settings
}

async fn load_settings_from_disk() -> Settings {
    let Some(path) = crate::paths::settings_path() else {
        tracing::warn!("Settings path requested before app paths were initialized");
        return Settings::default();
    };

    match fs::read_to_string(&path).await {
        Ok(contents) => parse_settings_json(&contents),
        Err(error) => {
            tracing::info!("Settings file not loaded, using defaults: {}", error);
            Settings::default()
        }
    }
}

/// Sync version of load_settings for use in menu building
pub fn load_settings_sync() -> Settings {
    // Check cache first
    if let Ok(cache) = SETTINGS_CACHE.read() {
        if let Some(ref settings) = *cache {
            return settings.clone();
        }
    }

    // Load from disk
    let Some(path) = crate::paths::settings_path() else {
        tracing::warn!("Settings path requested before app paths were initialized");
        return Settings::default();
    };
    let settings = match std::fs::read_to_string(&path) {
        Ok(contents) => parse_settings_json(&contents),
        Err(error) => {
            tracing::info!("Settings file not loaded, using defaults: {}", error);
            Settings::default()
        }
    };

    // Update cache
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = Some(settings.clone());
    }

    settings
}

pub async fn save_settings(settings: &Settings) -> Result<(), String> {
    let _guard = SETTINGS_UPDATE_LOCK.lock().await;
    save_settings_unlocked(settings).await
}

async fn save_settings_unlocked(settings: &Settings) -> Result<(), String> {
    let sanitized = sanitize_settings(settings.clone());
    let path =
        crate::paths::settings_path().ok_or_else(|| "App paths not initialized".to_string())?;

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create settings directory: {e}"))?;
    }

    let json = serde_json::to_string_pretty(&sanitized)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;

    fs::write(&path, json)
        .await
        .map_err(|e| format!("Failed to write settings: {e}"))?;

    // Update cache with new settings
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = Some(sanitized);
    }

    Ok(())
}

pub async fn update_settings_atomic<F>(mutator: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    let _guard = SETTINGS_UPDATE_LOCK.lock().await;
    let mut settings = load_settings_from_disk().await;
    mutator(&mut settings);
    let sanitized = sanitize_settings(settings);
    save_settings_unlocked(&sanitized).await?;
    Ok(sanitized)
}

/// Invalidate the settings cache (call after external changes)
#[allow(dead_code)]
pub fn invalidate_settings_cache() {
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = None;
    }
}

#[tauri::command]
pub async fn get_settings() -> Result<Settings, String> {
    Ok(load_settings().await)
}

pub async fn update_settings(settings: Settings) -> Result<Settings, String> {
    let _guard = SETTINGS_UPDATE_LOCK.lock().await;
    let latest = load_settings_from_disk().await;
    let merged = merge_settings_update(latest, settings);
    save_settings_unlocked(&merged).await?;
    Ok(merged)
}

fn sanitize_settings(mut settings: Settings) -> Settings {
    settings.dictionary_terms = crate::dictionary::sanitize_terms(&settings.dictionary_terms);
    settings
}

fn parse_settings_json(contents: &str) -> Settings {
    let value: serde_json::Value = match serde_json::from_str(contents) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!("Failed to parse settings JSON, using defaults: {}", error);
            return Settings::default();
        }
    };

    let raw_onboarding_completed = value
        .get("onboardingCompleted")
        .and_then(serde_json::Value::as_bool);
    let raw_dictionary_terms = value
        .get("dictionaryTerms")
        .and_then(serde_json::Value::as_array)
        .map(|terms| {
            terms
                .iter()
                .filter_map(|term| term.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });

    let mut settings = match serde_json::from_value::<Settings>(value) {
        Ok(settings) => sanitize_settings(settings),
        Err(error) => {
            tracing::warn!(
                "Failed to deserialize settings JSON, using defaults: {}",
                error
            );
            Settings::default()
        }
    };

    if raw_onboarding_completed == Some(true) && !settings.onboarding_completed {
        tracing::warn!("Recovering onboardingCompleted=true from raw settings JSON");
        settings.onboarding_completed = true;
    }

    if settings.dictionary_terms.is_empty() {
        if let Some(raw_terms) = raw_dictionary_terms {
            let sanitized_terms = crate::dictionary::sanitize_terms(&raw_terms);
            if !sanitized_terms.is_empty() {
                tracing::warn!(
                    "Recovering {} dictionary terms from raw settings JSON",
                    sanitized_terms.len()
                );
                settings.dictionary_terms = sanitized_terms;
            }
        }
    }

    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::runtime::Builder;

    static SETTINGS_TEST_MUTEX: StdMutex<()> = StdMutex::new(());

    fn init_test_paths() -> std::path::PathBuf {
        let path = std::env::temp_dir().join("fing-settings-tests");
        crate::paths::init_test_app_data_dir(path.clone());
        path
    }

    async fn reset_test_settings() {
        let app_data_dir = init_test_paths();
        let settings_path = crate::paths::settings_path().expect("settings path should exist");

        invalidate_settings_cache();
        let _ = fs::remove_file(&settings_path).await;
        fs::create_dir_all(&app_data_dir)
            .await
            .expect("test app data dir should be created");
    }

    fn run_async_test<F>(test: F)
    where
        F: std::future::Future<Output = ()>,
    {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(test);
    }

    fn unique_suffix() -> String {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "0".to_string())
    }

    #[test]
    fn update_settings_preserves_onboarding_completed_when_incoming_snapshot_is_stale() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX
                .lock()
                .expect("settings test mutex should lock");
            reset_test_settings().await;

            let completed = Settings {
                onboarding_completed: true,
                ..Settings::default()
            };
            save_settings(&completed)
                .await
                .expect("completed settings should save");

            let stale = Settings {
                onboarding_completed: false,
                selected_microphone_id: Some(format!("mic-{}", unique_suffix())),
                ..Settings::default()
            };

            let updated = update_settings(stale)
                .await
                .expect("stale settings update should succeed");

            assert!(updated.onboarding_completed);
            assert!(updated.selected_microphone_id.is_some());

            let reloaded = load_settings_from_disk().await;
            assert!(reloaded.onboarding_completed);
            assert_eq!(
                reloaded.selected_microphone_id,
                updated.selected_microphone_id
            );
        });
    }

    #[test]
    fn update_settings_keeps_onboarding_completed_sticky() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX
                .lock()
                .expect("settings test mutex should lock");
            reset_test_settings().await;

            let persisted = Settings {
                onboarding_completed: true,
                ..Settings::default()
            };
            save_settings(&persisted)
                .await
                .expect("completed settings should save");

            let incoming = Settings {
                onboarding_completed: false,
                theme: Theme::Dark,
                ..Settings::default()
            };

            let updated = update_settings(incoming)
                .await
                .expect("settings update should succeed");

            assert!(updated.onboarding_completed);
            assert_eq!(updated.theme, Theme::Dark);
        });
    }

    #[test]
    fn update_settings_keeps_incoming_completed_state_when_disk_read_is_incomplete() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX
                .lock()
                .expect("settings test mutex should lock");
            reset_test_settings().await;

            save_settings(&Settings::default())
                .await
                .expect("default settings should save");

            let incoming = Settings {
                onboarding_completed: true,
                dictionary_terms: vec!["Tauri".to_string()],
                theme: Theme::Light,
                ..Settings::default()
            };

            let updated = update_settings(incoming)
                .await
                .expect("settings update should succeed");

            assert!(updated.onboarding_completed);
            assert_eq!(updated.theme, Theme::Light);
            assert_eq!(updated.dictionary_terms, vec!["Tauri".to_string()]);
        });
    }

    #[test]
    fn update_settings_still_allows_incomplete_onboarding_settings_updates() {
        run_async_test(async {
            let _guard = SETTINGS_TEST_MUTEX
                .lock()
                .expect("settings test mutex should lock");
            reset_test_settings().await;

            save_settings(&Settings::default())
                .await
                .expect("default settings should save");

            let incoming = Settings {
                selected_microphone_id: Some(format!("mic-{}", unique_suffix())),
                ..Settings::default()
            };

            let updated = update_settings(incoming)
                .await
                .expect("settings update should succeed");

            assert!(!updated.onboarding_completed);
            assert!(updated.selected_microphone_id.is_some());

            let reloaded = load_settings_from_disk().await;
            assert!(!reloaded.onboarding_completed);
            assert_eq!(
                reloaded.selected_microphone_id,
                updated.selected_microphone_id
            );
        });
    }
}
