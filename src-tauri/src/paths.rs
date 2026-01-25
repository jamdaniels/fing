use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::Manager;

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the app data directory from Tauri's path resolver.
/// Must be called once during app setup.
pub fn init(app: &tauri::App) -> Result<(), String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    APP_DATA_DIR
        .set(path)
        .map_err(|_| "App data dir already initialized".to_string())
}

/// Get the app data directory. Panics if not initialized.
pub fn app_data_dir() -> &'static PathBuf {
    APP_DATA_DIR
        .get()
        .expect("App data dir not initialized - call paths::init() first")
}

pub fn db_path() -> PathBuf {
    app_data_dir().join("transcripts.db")
}

pub fn settings_path() -> PathBuf {
    app_data_dir().join("settings.json")
}

pub fn models_dir() -> PathBuf {
    app_data_dir().join("models")
}
