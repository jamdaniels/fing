use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        RwLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

const UPDATE_STATUS_CHANGED_EVENT: &str = "update-status-changed";
const UPDATE_CHECK_INTERVAL_SECS: u64 = 12 * 60 * 60;
const CURRENT_APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub update_available: bool,
    pub checking: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub update_available: bool,
    pub available_version: Option<String>,
    pub available_body: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedUpdateState {
    #[serde(default)]
    update_available: bool,
    #[serde(default)]
    detected_for_app_version: Option<String>,
    #[serde(default)]
    last_checked_at: Option<u64>,
}

#[derive(Debug, Default)]
struct RuntimeUpdateState {
    enabled: bool,
    update_available: bool,
    detected_for_app_version: Option<String>,
    last_checked_at: Option<u64>,
    checking: bool,
}

static UPDATE_STATE: Lazy<RwLock<RuntimeUpdateState>> =
    Lazy::new(|| RwLock::new(RuntimeUpdateState::default()));
static UPDATE_CHECK_LOCK: Lazy<tokio::sync::Mutex<()>> =
    Lazy::new(|| tokio::sync::Mutex::new(()));
static STOP_PERIODIC_CHECKS: Lazy<tokio::sync::Notify> = Lazy::new(tokio::sync::Notify::new);
static PERIODIC_TASK_RUNNING: AtomicBool = AtomicBool::new(false);

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn sanitized_persisted_state(state: PersistedUpdateState) -> PersistedUpdateState {
    if state.detected_for_app_version.as_deref() == Some(CURRENT_APP_VERSION) {
        state
    } else {
        PersistedUpdateState::default()
    }
}

fn should_check_for_current_app_version_from_state(
    enabled: bool,
    update_available: bool,
    detected_for_app_version: Option<&str>,
) -> bool {
    enabled && !(update_available && detected_for_app_version == Some(CURRENT_APP_VERSION))
}

fn read_persisted_state_from_disk() -> Result<PersistedUpdateState, String> {
    let Some(path) = crate::paths::update_state_path() else {
        return Ok(PersistedUpdateState::default());
    };

    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PersistedUpdateState::default());
        }
        Err(error) => {
            return Err(format!("Failed to read persisted update state: {error}"));
        }
    };

    let parsed = serde_json::from_str::<PersistedUpdateState>(&contents)
        .map_err(|error| format!("Failed to parse persisted update state: {error}"))?;

    Ok(parsed)
}

fn write_persisted_state_to_disk(state: &PersistedUpdateState) -> Result<(), String> {
    let Some(path) = crate::paths::update_state_path() else {
        return Err("App paths not initialized".to_string());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create update state directory: {error}"))?;
    }

    let json = serde_json::to_string_pretty(state)
        .map_err(|error| format!("Failed to serialize update state: {error}"))?;

    fs::write(path, json).map_err(|error| format!("Failed to write update state: {error}"))
}

fn runtime_status() -> UpdateStatus {
    let state = match UPDATE_STATE.read() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Update state read lock poisoned, recovering");
            poisoned.into_inner()
        }
    };

    UpdateStatus {
        update_available: state.update_available,
        checking: state.checking,
    }
}

fn persisted_state_snapshot() -> PersistedUpdateState {
    let state = match UPDATE_STATE.read() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Update state read lock poisoned while snapshotting, recovering");
            poisoned.into_inner()
        }
    };

    PersistedUpdateState {
        update_available: state.update_available,
        detected_for_app_version: state.detected_for_app_version.clone(),
        last_checked_at: state.last_checked_at,
    }
}

fn set_checking(is_checking: bool) {
    let mut state = match UPDATE_STATE.write() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Update state write lock poisoned while setting checking, recovering");
            poisoned.into_inner()
        }
    };
    state.checking = is_checking;
}

fn apply_persisted_state(state: PersistedUpdateState) -> Result<(), String> {
    let needs_persist = state.detected_for_app_version.is_some()
        && state.detected_for_app_version.as_deref() != Some(CURRENT_APP_VERSION);
    let sanitized = sanitized_persisted_state(state);

    {
        let mut runtime = match UPDATE_STATE.write() {
            Ok(runtime) => runtime,
            Err(poisoned) => {
                tracing::warn!("Update state write lock poisoned while applying state, recovering");
                poisoned.into_inner()
            }
        };
        runtime.update_available = sanitized.update_available;
        runtime.detected_for_app_version = sanitized.detected_for_app_version.clone();
        runtime.last_checked_at = sanitized.last_checked_at;
        runtime.checking = false;
    }

    if needs_persist {
        write_persisted_state_to_disk(&PersistedUpdateState::default())?;
    }

    Ok(())
}

fn emit_status_changed(app: &AppHandle) {
    let status = runtime_status();

    if let Err(error) = crate::rebuild_tray_menu(app) {
        tracing::warn!("Failed to rebuild tray menu after update status change: {error}");
    }

    if let Err(error) = app.emit(UPDATE_STATUS_CHANGED_EVENT, status) {
        tracing::warn!("Failed to emit update status event: {error}");
    }
}

fn mark_enabled(enabled: bool) {
    let mut state = match UPDATE_STATE.write() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Update state write lock poisoned while toggling enablement, recovering");
            poisoned.into_inner()
        }
    };
    state.enabled = enabled;
}

fn should_check_for_current_app_version() -> bool {
    let state = match UPDATE_STATE.read() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Update state read lock poisoned while deciding check eligibility, recovering");
            poisoned.into_inner()
        }
    };

    should_check_for_current_app_version_from_state(
        state.enabled,
        state.update_available,
        state.detected_for_app_version.as_deref(),
    )
}

fn update_menu_label() -> &'static str {
    let state = match UPDATE_STATE.read() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("Update state read lock poisoned while reading menu label, recovering");
            poisoned.into_inner()
        }
    };

    if state.update_available {
        "Update Available"
    } else {
        "Check for Updates"
    }
}

pub fn tray_menu_label() -> &'static str {
    update_menu_label()
}

pub fn current_update_status() -> UpdateStatus {
    runtime_status()
}

pub async fn initialize_for_ready_app() -> Result<(), String> {
    mark_enabled(true);
    let persisted = read_persisted_state_from_disk()?;
    apply_persisted_state(persisted)?;
    Ok(())
}

pub fn start_periodic_checks(app: &AppHandle) {
    if !should_check_for_current_app_version() {
        return;
    }

    if PERIODIC_TASK_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let wait_result = tokio::time::timeout(
                std::time::Duration::from_secs(UPDATE_CHECK_INTERVAL_SECS),
                STOP_PERIODIC_CHECKS.notified(),
            )
            .await;

            if wait_result.is_ok() {
                break;
            }

            if !should_check_for_current_app_version() {
                break;
            }

            let _ = run_check(&app_handle).await;

            if !should_check_for_current_app_version() {
                break;
            }
        }

        PERIODIC_TASK_RUNNING.store(false, Ordering::SeqCst);
    });
}

fn stop_periodic_checks() {
    if PERIODIC_TASK_RUNNING.load(Ordering::SeqCst) {
        STOP_PERIODIC_CHECKS.notify_one();
    }
}

fn sync_runtime_from_check_result(update_available: bool) -> Result<(), String> {
    let mut runtime = match UPDATE_STATE.write() {
        Ok(runtime) => runtime,
        Err(poisoned) => {
            tracing::warn!("Update state write lock poisoned while syncing check result, recovering");
            poisoned.into_inner()
        }
    };

    runtime.last_checked_at = Some(current_timestamp());
    runtime.detected_for_app_version = Some(CURRENT_APP_VERSION.to_string());
    runtime.update_available = update_available;

    let persisted = PersistedUpdateState {
        update_available: runtime.update_available,
        detected_for_app_version: runtime.detected_for_app_version.clone(),
        last_checked_at: runtime.last_checked_at,
    };
    drop(runtime);

    write_persisted_state_to_disk(&persisted)
}

async fn run_check(app: &AppHandle) -> Result<UpdateStatus, String> {
    {
        let state = match UPDATE_STATE.read() {
            Ok(state) => state,
            Err(poisoned) => {
                tracing::warn!("Update state read lock poisoned before running check, recovering");
                poisoned.into_inner()
            }
        };

        if !state.enabled {
            return Err("Update checks are unavailable during setup".to_string());
        }

        if state.update_available
            && state.detected_for_app_version.as_deref() == Some(CURRENT_APP_VERSION)
        {
            return Ok(runtime_status());
        }
    }

    let _guard = UPDATE_CHECK_LOCK.lock().await;

    if !should_check_for_current_app_version() {
        return Ok(runtime_status());
    }

    set_checking(true);
    emit_status_changed(app);

    let updater = app.updater().map_err(|error| error.to_string())?;
    let check_result = updater.check().await;

    set_checking(false);

    match check_result {
        Ok(Some(_update)) => {
            sync_runtime_from_check_result(true)?;
            stop_periodic_checks();
            emit_status_changed(app);
            Ok(runtime_status())
        }
        Ok(None) => {
            sync_runtime_from_check_result(false)?;
            emit_status_changed(app);
            Ok(runtime_status())
        }
        Err(error) => {
            let message = error.to_string();
            emit_status_changed(app);
            tracing::warn!("Silent update check failed: {message}");
            Ok(runtime_status())
        }
    }
}

pub fn schedule_startup_check(app: &AppHandle) {
    if !should_check_for_current_app_version() {
        return;
    }

    start_periodic_checks(app);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_check(&app_handle).await;
    });
}

pub async fn enable_after_onboarding(app: &AppHandle) -> Result<UpdateStatus, String> {
    initialize_for_ready_app().await?;

    if !should_check_for_current_app_version() {
        emit_status_changed(app);
        return Ok(runtime_status());
    }

    start_periodic_checks(app);
    run_check(app).await
}

#[tauri::command]
pub fn get_update_status() -> UpdateStatus {
    current_update_status()
}

#[tauri::command]
pub async fn check_for_updates_now(app: AppHandle) -> Result<UpdateCheckResult, String> {
    {
        let state = match UPDATE_STATE.read() {
            Ok(state) => state,
            Err(poisoned) => {
                tracing::warn!("Update state read lock poisoned before manual update check, recovering");
                poisoned.into_inner()
            }
        };

        if !state.enabled {
            return Err("Update checks are unavailable during setup".to_string());
        }
    }

    let _guard = UPDATE_CHECK_LOCK.lock().await;

    set_checking(true);
    emit_status_changed(&app);

    let updater = app.updater().map_err(|error| error.to_string())?;
    let check_result = updater.check().await;

    set_checking(false);

    match check_result {
        Ok(Some(update)) => {
            sync_runtime_from_check_result(true)?;
            stop_periodic_checks();
            emit_status_changed(&app);
            Ok(UpdateCheckResult {
                update_available: true,
                available_version: Some(update.version),
                available_body: update.body,
            })
        }
        Ok(None) => {
            sync_runtime_from_check_result(false)?;
            emit_status_changed(&app);
            Ok(UpdateCheckResult::default())
        }
        Err(error) => {
            emit_status_changed(&app);
            Err(error.to_string())
        }
    }
}

#[tauri::command]
pub async fn clear_update_status(app: AppHandle) -> Result<UpdateStatus, String> {
    {
        let mut state = match UPDATE_STATE.write() {
            Ok(state) => state,
            Err(poisoned) => {
                tracing::warn!("Update state write lock poisoned while clearing state, recovering");
                poisoned.into_inner()
            }
        };

        if !state.enabled {
            return Ok(UpdateStatus::default());
        }

        state.update_available = false;
        state.detected_for_app_version = Some(CURRENT_APP_VERSION.to_string());
        state.last_checked_at = Some(current_timestamp());
        state.checking = false;
    }

    write_persisted_state_to_disk(&persisted_state_snapshot())?;
    emit_status_changed(&app);

    if should_check_for_current_app_version() {
        start_periodic_checks(&app);
    }

    Ok(runtime_status())
}

#[cfg(test)]
mod tests {
    use super::{
        read_persisted_state_from_disk, sanitized_persisted_state,
        should_check_for_current_app_version_from_state, write_persisted_state_to_disk,
        PersistedUpdateState, CURRENT_APP_VERSION,
    };
    use std::{env, fs, path::PathBuf, sync::Once};

    static TEST_PATH_INIT: Once = Once::new();

    fn set_test_update_state_path(path: PathBuf) {
        TEST_PATH_INIT.call_once(|| {
            crate::paths::init_test_update_state_path(path);
        });
    }

    fn unique_test_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);

        env::temp_dir().join(format!("fing-{name}-{nanos}.json"))
    }

    #[test]
    fn persisted_state_round_trip() {
        let path = unique_test_path("update-state-round-trip");
        set_test_update_state_path(path.clone());

        let expected = PersistedUpdateState {
            update_available: true,
            detected_for_app_version: Some(CURRENT_APP_VERSION.to_string()),
            last_checked_at: Some(123),
        };

        write_persisted_state_to_disk(&expected).expect("persisted state should write");
        let actual = read_persisted_state_from_disk().expect("persisted state should read");

        assert_eq!(actual.update_available, expected.update_available);
        assert_eq!(
            actual.detected_for_app_version,
            expected.detected_for_app_version
        );
        assert_eq!(actual.last_checked_at, expected.last_checked_at);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn stale_cached_update_is_cleared_for_new_app_version() {
        let stale = PersistedUpdateState {
            update_available: true,
            detected_for_app_version: Some("0.0.1".to_string()),
            last_checked_at: Some(123),
        };

        let sanitized = sanitized_persisted_state(stale);
        assert!(!sanitized.update_available);
        assert!(sanitized.detected_for_app_version.is_none());
        assert!(sanitized.last_checked_at.is_none());
    }

    #[test]
    fn should_check_skips_when_sticky_update_exists_for_current_version() {
        assert!(!should_check_for_current_app_version_from_state(
            true,
            true,
            Some(CURRENT_APP_VERSION)
        ));
        assert!(should_check_for_current_app_version_from_state(
            true,
            false,
            Some(CURRENT_APP_VERSION)
        ));
        assert!(should_check_for_current_app_version_from_state(
            true,
            true,
            Some("0.0.1")
        ));
        assert!(!should_check_for_current_app_version_from_state(
            false,
            false,
            Some(CURRENT_APP_VERSION)
        ));
    }
}
