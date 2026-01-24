use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CommandOrControl+Shift+Space".to_string(),
            model_path: String::new(),
            selected_microphone_id: None,
            auto_start: false,
            sound_enabled: true,
            paste_enabled: true,
            history_enabled: true,
            history_limit: 1000,
        }
    }
}

fn get_settings_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fing");
    data_dir.join("settings.json")
}

pub async fn load_settings() -> Settings {
    let path = get_settings_path();

    if let Ok(contents) = fs::read_to_string(&path).await {
        serde_json::from_str(&contents).unwrap_or_default()
    } else {
        Settings::default()
    }
}

pub async fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = get_settings_path();

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

    Ok(())
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
