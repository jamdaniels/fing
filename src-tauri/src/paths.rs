use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock};
use tauri::Manager;
use tokio::sync::Notify;

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static INIT_NOTIFY: LazyLock<Notify> = LazyLock::new(Notify::new);
#[cfg(test)]
static TEST_APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
#[cfg(test)]
static TEST_UPDATE_STATE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Initialize the app data directory from Tauri's path resolver.
/// Must be called once during app setup.
pub fn init(app: &tauri::App) -> Result<(), String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {e}"))?;

    APP_DATA_DIR
        .set(path)
        .map_err(|_| "App data dir already initialized".to_string())?;
    INIT_NOTIFY.notify_waiters();
    Ok(())
}

/// REGRESSION GUARD: Tauri creates the webview BEFORE `setup()` runs, and on
/// Windows the bundled frontend loads fast enough that its first IPC calls
/// (get_settings, get_bootstrap_status) arrive before `init` has stored the
/// app data dir. Answering those calls from an uninitialized state returned
/// default settings (onboardingCompleted=false, English UI) and re-showed
/// onboarding on every launch of an installed build. Settings/bootstrap reads
/// must await this before touching paths. Do not remove the wait or answer
/// path-dependent IPC before initialization.
pub async fn wait_until_initialized() {
    loop {
        // Create the listener before checking, so a notify between the check
        // and the await cannot be lost.
        let notified = INIT_NOTIFY.notified();
        if app_data_dir().is_some() {
            return;
        }
        notified.await;
    }
}

/// Get the app data directory. Returns None if not initialized.
pub fn app_data_dir() -> Option<&'static PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_APP_DATA_DIR.get() {
        return Some(path);
    }

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

/// Path to the persisted update-check state file. Returns None if paths not initialized.
pub fn update_state_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_UPDATE_STATE_PATH.get() {
        return Some(path.clone());
    }

    app_data_dir().map(|p| p.join("update_state.json"))
}

/// Directory containing downloaded model files. Returns None if paths not initialized.
pub fn models_dir() -> Option<PathBuf> {
    app_data_dir().map(|p| p.join("models"))
}

/// Log directory inside the app data dir (used by Windows file logging,
/// where stdout is invisible in release builds). Returns None if paths not
/// initialized.
#[cfg(target_os = "windows")]
pub fn log_dir() -> Option<PathBuf> {
    app_data_dir().map(|p| p.join("logs"))
}

#[cfg(test)]
pub fn init_test_app_data_dir(path: PathBuf) {
    let _ = TEST_APP_DATA_DIR.set(path);
    INIT_NOTIFY.notify_waiters();
}

#[cfg(test)]
pub fn init_test_update_state_path(path: PathBuf) {
    let _ = TEST_UPDATE_STATE_PATH.set(path);
}
