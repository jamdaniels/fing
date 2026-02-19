use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

fn show_notification(app: &AppHandle, title: &str, message: &str) {
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(message)
        .show()
    {
        tracing::warn!("Failed to show notification: {}", e);
    }
}

/// Show error notification
pub fn show_error(app: &AppHandle, title: &str, message: &str) {
    show_notification(app, title, message);
    tracing::error!("Error notification: {} - {}", title, message);
}

/// Show informational notification
pub fn show_info(app: &AppHandle, title: &str, message: &str) {
    show_notification(app, title, message);
    tracing::info!("Info notification: {} - {}", title, message);
}

#[tauri::command]
pub fn notify_error(app: AppHandle, title: String, message: String) {
    show_error(&app, &title, &message);
}
