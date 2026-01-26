use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use tokio::fs;

/// Cached settings to reduce disk I/O
static SETTINGS_CACHE: RwLock<Option<Settings>> = RwLock::new(None);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub hotkey: String,
    pub model_path: String,
    pub selected_microphone_id: Option<String>,
    pub auto_start: bool,
    pub sound_enabled: bool,
    pub paste_enabled: bool,
    pub history_enabled: bool,
    pub history_limit: i64,
    #[serde(default)]
    pub onboarding_completed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "F8".to_string(),
            model_path: String::new(),
            selected_microphone_id: None,
            auto_start: false,
            sound_enabled: true,
            paste_enabled: true,
            history_enabled: true,
            history_limit: 1000,
            onboarding_completed: false,
        }
    }
}

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
    let path = crate::paths::settings_path();

    if let Ok(contents) = fs::read_to_string(&path).await {
        serde_json::from_str(&contents).unwrap_or_default()
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
    let path = crate::paths::settings_path();
    let settings = if let Ok(contents) = std::fs::read_to_string(&path) {
        serde_json::from_str(&contents).unwrap_or_default()
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
    let path = crate::paths::settings_path();

    // Ensure directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create settings directory: {}", e))?;
    }

    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    fs::write(&path, json)
        .await
        .map_err(|e| format!("Failed to write settings: {}", e))?;

    // Update cache with new settings
    if let Ok(mut cache) = SETTINGS_CACHE.write() {
        *cache = Some(settings.clone());
    }

    Ok(())
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

#[tauri::command]
pub async fn update_settings(settings: Settings) -> Result<Settings, String> {
    save_settings(&settings).await?;
    Ok(settings)
}
