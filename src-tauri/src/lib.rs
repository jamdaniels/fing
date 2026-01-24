// Fing - Fast, private, local speech-to-text

mod app_info;
mod audio;
mod db;
mod engine;
mod hotkey;
mod indicator;
mod model;
mod notifications;
mod paste;
mod platform;
mod settings;
mod sounds;
mod state;
mod stats;
mod transcribe;
mod updates;

use audio::{AudioCapture, AudioDevice, MicrophoneTest};
use state::AppState;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

// TODO: Audio capture needs to run on dedicated thread (cpal::Stream is !Send)
// For now, audio commands create temporary AudioCapture instances
// Future: Use channels to communicate with audio thread

#[tauri::command]
fn get_app_state() -> String {
    let state = state::APP_STATE.read().unwrap();
    state.as_str().to_string()
}

#[tauri::command]
fn get_audio_devices() -> Vec<AudioDevice> {
    AudioCapture::list_devices()
}

#[tauri::command]
fn set_audio_device(_device_id: Option<String>) -> Result<(), String> {
    // TODO: Store device preference in settings
    // Device selection will be used when initializing capture on recording
    Ok(())
}

#[tauri::command]
fn test_microphone() -> Result<MicrophoneTest, String> {
    // Create temporary capture for testing
    let mut capture = AudioCapture::new();
    capture.test_microphone().map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_model_download() -> Result<String, String> {
    model::download().await.map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn get_download_progress() -> model::DownloadProgress {
    model::get_progress()
}

#[tauri::command]
fn verify_model(path: String) -> model::ModelVerification {
    model::verify(std::path::Path::new(&path))
}

#[tauri::command]
fn select_model_file(app: tauri::AppHandle) -> Option<String> {
    model::select_file(&app).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn open_main_window(app: tauri::AppHandle, tab: Option<String>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;

        // Emit navigation event if tab specified
        if let Some(tab_name) = tab {
            app.emit("navigate-to-tab", tab_name)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    tracing::info!("Application shutdown requested");
    app.exit(0);
}

#[tauri::command]
fn set_auto_start(enabled: bool) -> Result<(), String> {
    if enabled {
        platform::enable_auto_start()
    } else {
        platform::disable_auto_start()
    }
}

#[tauri::command]
fn get_auto_start() -> bool {
    platform::is_auto_start_enabled()
}

#[tauri::command]
fn complete_setup(app: tauri::AppHandle) -> Result<(), String> {
    // Verify model exists at default path
    let model_path = model::default_model_path();
    let verification = model::verify(&model_path);

    if !verification.is_valid {
        return Err(format!(
            "Model not valid at {}: exists={}, size_valid={}, hash_valid={}",
            verification.path,
            verification.exists,
            verification.size_valid,
            verification.hash_valid
        ));
    }

    // Initialize the transcriber
    let model_path_str = model_path.to_string_lossy().to_string();
    transcribe::init_transcriber(&model_path_str)
        .map_err(|e| format!("Failed to initialize transcriber: {:?}", e))?;

    // Transition to Ready state (this also emits app-state-changed event)
    state::set_state(&app, AppState::Ready)?;

    tracing::info!("Setup complete, app is ready");
    Ok(())
}

fn build_tray_menu(app: &tauri::App) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let current_state = state::get_state();

    if current_state == AppState::NeedsSetup {
        // Simplified menu for setup state
        let setup = MenuItem::with_id(app, "setup", "Complete Setup...", true, None::<&str>)?;
        let separator = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

        Ok(Menu::with_items(app, &[&setup, &separator, &quit])?)
    } else {
        // Full menu for normal operation
        let open = MenuItem::with_id(app, "open", "Open App", true, None::<&str>)?;
        let separator1 = PredefinedMenuItem::separator(app)?;

        // Build microphone submenu
        let mic_submenu = build_mic_submenu(app)?;

        let separator2 = PredefinedMenuItem::separator(app)?;
        let updates = MenuItem::with_id(app, "check_updates", "Check for Updates", true, None::<&str>)?;
        let about = MenuItem::with_id(app, "about", "About", true, None::<&str>)?;
        let separator3 = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

        Ok(Menu::with_items(
            app,
            &[&open, &separator1, &mic_submenu, &separator2, &updates, &about, &separator3, &quit],
        )?)
    }
}

fn build_mic_submenu(app: &tauri::App) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let devices = AudioCapture::list_devices();

    let mut mic_items: Vec<MenuItem<tauri::Wry>> = Vec::new();

    // Add system default option
    let default_item = MenuItem::with_id(
        app,
        "mic_default",
        "System Default",
        true,
        None::<&str>,
    )?;
    mic_items.push(default_item);

    // Add each device
    for device in devices {
        let item_id = format!("mic_{}", device.id.replace(' ', "_"));
        let label = if device.is_default {
            format!("{} (Default)", device.name)
        } else {
            device.name.clone()
        };
        let item = MenuItem::with_id(app, &item_id, &label, true, None::<&str>)?;
        mic_items.push(item);
    }

    let mic_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        mic_items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect();

    Ok(Submenu::with_items(app, "Select Mic", true, &mic_refs)?)
}

fn handle_menu_event(app: &tauri::AppHandle, event_id: &str) {
    match event_id {
        "quit" => {
            tracing::info!("Quit requested from tray");
            app.exit(0);
        }
        "open" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "setup" => {
            // Open main window for onboarding
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "check_updates" => {
            // Open main window and navigate to settings/updates
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = app.emit("navigate-to-tab", "settings");
            }
        }
        "about" => {
            // Open main window and navigate to about tab
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = app.emit("navigate-to-tab", "about");
            }
        }
        id if id.starts_with("mic_") => {
            let device_id = if id == "mic_default" {
                None
            } else {
                // Extract device ID from menu ID
                Some(id.strip_prefix("mic_").unwrap().replace('_', " "))
            };
            tracing::info!("Microphone changed via tray: {:?}", device_id);
            // TODO: Store in settings and apply to audio capture
        }
        _ => {
            tracing::debug!("Unknown menu event: {}", event_id);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Initialize database
            if let Err(e) = db::init_db() {
                tracing::error!("Failed to initialize database: {}", e);
            }

            // Build tray menu based on app state
            let menu = build_tray_menu(app)?;

            // Create tray icon
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| {
                    handle_menu_event(app, event.id.as_ref());
                })
                .build(app)?;

            // Show main window on first launch
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }

            // Register global hotkey and transition to Ready state
            let app_handle = app.handle().clone();
            if let Err(e) = hotkey::register_hotkey(&app_handle) {
                tracing::error!("Failed to register hotkey: {}", e);
            } else {
                // Transition to Ready state
                state::set_state(&app_handle, state::AppState::Ready).ok();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            // App info
            app_info::get_app_info,
            // Settings
            settings::get_settings,
            settings::update_settings,
            // Stats
            stats::get_stats,
            // Database operations
            db::db_save_transcript,
            db::db_get_recent,
            db::db_search,
            db::db_delete,
            db::db_delete_all,
            // Audio
            get_audio_devices,
            set_audio_device,
            test_microphone,
            // Hotkey testing
            hotkey::test_transcription,
            // Indicator controls
            indicator::indicator_show_recording,
            indicator::indicator_show_processing,
            indicator::indicator_hide,
            // Notifications
            notifications::notify_clipboard_fallback,
            notifications::notify_error,
            // Window management
            open_main_window,
            quit_app,
            // Model management
            start_model_download,
            get_download_progress,
            verify_model,
            select_model_file,
            // Setup
            complete_setup,
            // Updates
            updates::check_for_updates_cmd,
            // Auto-start
            set_auto_start,
            get_auto_start,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
