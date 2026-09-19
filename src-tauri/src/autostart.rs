//! Start-on-login. Direct installs use the registry Run key / LaunchAgent via
//! tauri-plugin-autostart. Store (MSIX) installs must use the StartupTask
//! API: the Run key is virtualized inside the package container and the
//! install path changes with every Store update.

use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt as _;

pub async fn set_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    if crate::distribution::is_store_build() {
        return crate::platform::set_startup_task_enabled(enabled).await;
    }

    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|e| e.to_string())
    } else {
        autostart.disable().map_err(|e| e.to_string())
    }
}

pub async fn is_enabled(app: &AppHandle) -> bool {
    #[cfg(target_os = "windows")]
    if crate::distribution::is_store_build() {
        return crate::platform::is_startup_task_enabled()
            .await
            .unwrap_or(false);
    }

    app.autolaunch().is_enabled().unwrap_or(false)
}
