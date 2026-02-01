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

/// Get the app data directory. Returns None if not initialized.
pub fn app_data_dir() -> Option<&'static PathBuf> {
    APP_DATA_DIR.get()
}

/// Path to the SQLite database file. Returns None if paths not initialized.
pub fn db_path() -> Option<PathBuf> {
    app_data_dir().map(|p| p.join("transcripts.db"))
}

/// Path to the settings JSON file. Returns None if paths not initialized.
pub fn settings_path() -> Option<PathBuf> {
    app_data_dir().map(|p| p.join("settings.json"))
}

/// Directory containing downloaded model files. Returns None if paths not initialized.
pub fn models_dir() -> Option<PathBuf> {
    app_data_dir().map(|p| p.join("models"))
}
