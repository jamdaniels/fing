use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use std::time::Duration;

#[derive(Clone, serde::Serialize)]
pub struct IndicatorStatePayload {
    pub state: String,
}

/// Show indicator in recording state
pub fn show_recording(app: &AppHandle) -> Result<(), String> {
    let window = get_or_create_indicator(app)?;
    position_indicator(&window)?;
    window.show().map_err(|e| e.to_string())?;

    app.emit("indicator-state-changed", IndicatorStatePayload { state: "recording".to_string() })
        .map_err(|e| e.to_string())?;

    tracing::info!("Indicator showing: recording");
    Ok(())
}

/// Show indicator in processing state
pub fn show_processing(app: &AppHandle) -> Result<(), String> {
    app.emit("indicator-state-changed", IndicatorStatePayload { state: "processing".to_string() })
        .map_err(|e| e.to_string())?;

    tracing::info!("Indicator showing: processing");
    Ok(())
}

/// Hide the indicator window
pub fn hide(app: &AppHandle) -> Result<(), String> {
    app.emit("indicator-state-changed", IndicatorStatePayload { state: "hidden".to_string() })
        .map_err(|e| e.to_string())?;

    // Delay hide to allow shrink animation
    let app_handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        if let Some(window) = app_handle.get_webview_window("indicator") {
            let _ = window.hide();
        }
    });

    tracing::info!("Indicator hiding");
    Ok(())
}

/// Get existing indicator window or create it
fn get_or_create_indicator(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window("indicator") {
        return Ok(window);
    }

    // Window should already exist from tauri.conf.json, but if not, error
    Err("Indicator window not found".to_string())
}

/// Position indicator at bottom center of primary screen
pub fn position_indicator(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window
        .primary_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("No primary monitor found")?;

    let screen_size = monitor.size();
    let screen_position = monitor.position();
    let scale_factor = monitor.scale_factor();

    // Window dimensions from config
    let window_width = 70.0;
    let window_height = 30.0;
    let margin_bottom = 100.0;

    // Calculate position (convert physical pixels to logical)
    let screen_width = screen_size.width as f64 / scale_factor;
    let screen_height = screen_size.height as f64 / scale_factor;

    let x = screen_position.x as f64 / scale_factor + (screen_width - window_width) / 2.0;
    let y = screen_position.y as f64 / scale_factor + screen_height - window_height - margin_bottom;

    window
        .set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
        .map_err(|e| e.to_string())?;

    tracing::debug!("Indicator positioned at ({}, {})", x, y);
    Ok(())
}

// Tauri commands for testing/manual control
#[tauri::command]
pub fn indicator_show_recording(app: AppHandle) -> Result<(), String> {
    show_recording(&app)
}

#[tauri::command]
pub fn indicator_show_processing(app: AppHandle) -> Result<(), String> {
    show_processing(&app)
}

#[tauri::command]
pub fn indicator_hide(app: AppHandle) -> Result<(), String> {
    hide(&app)
}
