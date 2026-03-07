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
    pub onboarding_step: Option<u8>,
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
            onboarding_step: None,
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

    // Load from disk
    let settings = load_settings_from_disk().await;

    // Update cache
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = Some(settings.clone());
    }

    settings
}

async fn load_settings_from_disk() -> Settings {
    let Some(path) = crate::paths::settings_path() else {
        return Settings::default();
    };

    if let Ok(contents) = fs::read_to_string(&path).await {
        sanitize_settings(serde_json::from_str(&contents).unwrap_or_default())
    } else {
        Settings::default()
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
        return Settings::default();
    };
    let settings = if let Ok(contents) = std::fs::read_to_string(&path) {
        sanitize_settings(serde_json::from_str(&contents).unwrap_or_default())
    } else {
        Settings::default()
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
    let sanitized = sanitize_settings(settings);
    save_settings(&sanitized).await?;
    Ok(sanitized)
}

fn sanitize_settings(mut settings: Settings) -> Settings {
    settings.dictionary_terms = crate::dictionary::sanitize_terms(&settings.dictionary_terms);
    settings
}
