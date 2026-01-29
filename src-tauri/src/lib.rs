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
mod paths;
mod platform;
mod settings;
mod sounds;
mod state;
mod stats;
mod transcribe;
mod updates;

use audio::{AudioCapture, AudioDevice, MicrophoneTest};
use state::AppState;
use std::sync::Mutex;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    ActivationPolicy, Emitter, Manager,
};

/// Consolidated mic test state to prevent race conditions
/// All state changes go through a single lock acquisition
#[derive(Default)]
struct MicTestState {
    running: bool,
    level: u32,           // Fixed-point (level * 10000)
    receiving: bool,
    device_id: Option<String>,
}

lazy_static::lazy_static! {
    static ref MIC_TEST_STATE: Mutex<MicTestState> = Mutex::new(MicTestState::default());
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MicTestStartResult {
    pub requested_device: Option<String>,
    pub actual_device: String,
    pub device_matched: bool,
}

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
fn refresh_audio_devices() -> Vec<AudioDevice> {
    tracing::debug!("Refreshing audio device list");
    AudioCapture::list_devices()
}

#[tauri::command]
fn set_audio_device(_device_id: Option<String>) -> Result<(), String> {
    // TODO: Store device preference in settings
    // Device selection will be used when initializing capture on recording
    Ok(())
}

#[tauri::command]
fn test_microphone(device_id: Option<String>) -> Result<MicrophoneTest, String> {
    tracing::debug!("test_microphone called with device_id: {:?}", device_id);
    let mut capture = AudioCapture::new();
    if let Some(ref id) = device_id {
        tracing::debug!("Setting device to: {}", id);
        capture.set_device(Some(id.clone()));
    }
    let result = capture.test_microphone();
    tracing::debug!("test_microphone result: {:?}", result);
    result.map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_mic_test(device_id: Option<String>) -> Result<MicTestStartResult, String> {
    tracing::info!("Starting mic test with device: {:?}", device_id);

    // Stop any existing test and reset state atomically
    {
        let mut state = MIC_TEST_STATE
            .lock()
            .map_err(|e| format!("Mic test state lock poisoned: {}", e))?;
        state.running = false;
        state.level = 0;
        state.receiving = false;
        state.device_id = device_id.clone();
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Get device match info
    let mut capture = AudioCapture::new();
    if let Some(ref id) = device_id {
        capture.set_device(Some(id.clone()));
    }

    let match_result = match capture.start_mic_test() {
        Ok(result) => result,
        Err(e) => return Err(e.to_string()),
    };
    capture.stop_mic_test();

    let result = MicTestStartResult {
        requested_device: match_result.requested.clone(),
        actual_device: match_result.actual.clone(),
        device_matched: match_result.matched,
    };

    // Log result for debugging
    tracing::info!(
        "Mic test result: requested={:?}, actual='{}', matched={}",
        result.requested_device,
        result.actual_device,
        result.device_matched
    );
    if !match_result.matched {
        tracing::warn!(
            "Device mismatch! Requested device not found, using fallback."
        );
    }

    // Mark as running BEFORE spawning thread to avoid race condition
    {
        let mut state = MIC_TEST_STATE
            .lock()
            .map_err(|e| format!("Mic test state lock poisoned: {}", e))?;
        state.running = true;
    }

    // Start mic test thread (uses std::thread for blocking audio I/O)
    let device_id_clone = device_id.clone();
    std::thread::spawn(move || {
        let mut capture = AudioCapture::new();
        if let Some(id) = device_id_clone {
            capture.set_device(Some(id));
        }

        // Check if we should still run (stop might have been called already)
        let should_run = MIC_TEST_STATE.lock().map(|s| s.running).unwrap_or(false);
        if !should_run {
            tracing::info!("Mic test cancelled before init");
            return;
        }

        if let Err(e) = capture.init_capture() {
            tracing::error!("Failed to init mic test capture: {}", e);
            if let Ok(mut state) = MIC_TEST_STATE.lock() {
                state.running = false;
            }
            return;
        }

        tracing::info!("Mic test capture initialized, starting recording");

        // Start recording
        capture.begin_recording();

        loop {
            // Check if we should stop
            let should_run = MIC_TEST_STATE
                .lock()
                .map(|s| s.running)
                .unwrap_or(false);
            if !should_run {
                break;
            }

            std::thread::sleep(std::time::Duration::from_millis(50));

            // Get current level from buffer (this also clears it)
            let level_info = capture.get_mic_level();
            let level_fixed = (level_info.peak_level * 10000.0) as u32;

            // Update state atomically
            if let Ok(mut state) = MIC_TEST_STATE.lock() {
                state.level = level_fixed;
                state.receiving = level_info.is_receiving_audio;
            }

            if level_info.peak_level > 0.01 {
                tracing::debug!("Mic level: {:.4}", level_info.peak_level);
            }
        }

        capture.close_capture();
        tracing::info!("Mic test thread stopped");
    });

    Ok(result)
}

#[tauri::command]
fn get_mic_test_level() -> MicrophoneTest {
    let (level_fixed, is_receiving) = MIC_TEST_STATE
        .lock()
        .map(|s| (s.level, s.receiving))
        .unwrap_or((0, false));

    MicrophoneTest {
        device_name: String::new(),
        peak_level: level_fixed as f32 / 10000.0,
        is_receiving_audio: is_receiving,
    }
}

#[tauri::command]
fn stop_mic_test() {
    tracing::info!("Stopping mic test");
    if let Ok(mut state) = MIC_TEST_STATE.lock() {
        state.running = false;
        state.level = 0;
        state.receiving = false;
    }
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
fn check_model_exists() -> model::ModelVerification {
    let path = model::default_model_path();
    model::verify(&path)
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
fn check_accessibility_permission() -> bool {
    platform::check_accessibility_permission()
}

#[tauri::command]
fn request_accessibility_permission() -> bool {
    platform::request_accessibility_permission()
}

#[derive(serde::Serialize)]
struct PermissionStatus {
    microphone: String,
    accessibility: String,
}

#[tauri::command]
fn request_permissions() -> PermissionStatus {
    // Check microphone permission
    let mic_status = if cfg!(target_os = "macos") {
        platform::check_microphone_permission()
    } else {
        // On other platforms, assume granted if devices exist
        if AudioCapture::list_devices().is_empty() {
            "denied".to_string()
        } else {
            "granted".to_string()
        }
    };

    // Check accessibility
    let acc_status = if cfg!(target_os = "macos") {
        if platform::check_accessibility_permission() {
            "granted"
        } else {
            "denied"
        }
    } else {
        "not-applicable"
    };

    PermissionStatus {
        microphone: mic_status,
        accessibility: acc_status.to_string(),
    }
}

#[tauri::command]
fn request_microphone_permission() {
    #[cfg(target_os = "macos")]
    platform::request_microphone_permission();
}

#[tauri::command]
fn try_register_hotkey(app: tauri::AppHandle) -> Result<(), String> {
    hotkey::register_hotkey(&app)
}

#[tauri::command]
fn update_hotkey(hotkey: String) -> Result<(), String> {
    platform::set_hotkey(&hotkey)
}

#[tauri::command]
async fn complete_setup(app: tauri::AppHandle) -> Result<(), String> {
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

    // Save onboarding_completed to settings
    let mut current_settings = settings::load_settings().await;
    current_settings.onboarding_completed = true;
    settings::save_settings(&current_settings).await?;

    // Set hotkey from settings
    if let Err(e) = platform::set_hotkey(&current_settings.hotkey) {
        tracing::warn!("Failed to set hotkey: {}", e);
    }

    // Register the hotkey (creates the event tap)
    // Don't fail setup if this fails - user can fix permissions and restart
    if let Err(e) = hotkey::register_hotkey(&app) {
        tracing::warn!("Failed to register hotkey: {} - hotkey will work after app restart with proper permissions", e);
    }

    // Transition to Ready state (this also emits app-state-changed event)
    state::set_state(&app, AppState::Ready)?;

    // Rebuild tray menu to show full menu instead of "Complete Setup..."
    if let Err(e) = rebuild_tray_menu(&app) {
        tracing::error!("Failed to rebuild tray menu: {}", e);
    }

    tracing::info!("Setup complete, app is ready");
    Ok(())
}

fn build_tray_menu(app: &tauri::App) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let current_state = state::get_state();
    build_tray_menu_for_state(app, current_state)
}

fn build_tray_menu_for_state(app: &impl tauri::Manager<tauri::Wry>, current_state: AppState) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    if current_state == AppState::NeedsSetup {
        // Simplified menu for setup state
        let setup = MenuItem::with_id(app, "setup", "Complete Setup...", true, None::<&str>)?;
        let separator = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

        Ok(Menu::with_items(app, &[&setup, &separator, &quit])?)
    } else {
        // Full menu for normal operation
        let open = MenuItem::with_id(app, "open", "Open App", true, None::<&str>)?;
        let history = MenuItem::with_id(app, "history", "History", true, None::<&str>)?;
        let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
        let separator1 = PredefinedMenuItem::separator(app)?;

        // Build microphone submenu
        let mic_submenu = build_mic_submenu_for_handle(app)?;

        let separator2 = PredefinedMenuItem::separator(app)?;
        let updates = MenuItem::with_id(app, "check_updates", "Check for Updates", true, None::<&str>)?;
        let about = MenuItem::with_id(app, "about", "About", true, None::<&str>)?;
        let separator3 = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

        Ok(Menu::with_items(
            app,
            &[&open, &history, &settings, &separator1, &mic_submenu, &separator2, &updates, &about, &separator3, &quit],
        )?)
    }
}

/// Tray icon ID constant
const TRAY_ID: &str = "fing-tray";

/// Rebuild the tray menu based on current app state
fn rebuild_tray_menu(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let current_state = state::get_state();
    let menu = build_tray_menu_for_state(app, current_state)?;

    // Get the tray icon by ID and update its menu
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu))?;
        tracing::info!("Tray menu rebuilt for state: {:?}", current_state);
    } else {
        tracing::warn!("Could not find tray icon with ID: {}", TRAY_ID);
    }

    Ok(())
}

fn build_mic_submenu_for_handle(app: &impl tauri::Manager<tauri::Wry>) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let devices = AudioCapture::list_devices();
    let current_settings = settings::load_settings_sync();
    let selected_id = current_settings.selected_microphone_id;

    let mut mic_items: Vec<CheckMenuItem<tauri::Wry>> = Vec::new();

    // Add system default option (selected when selected_microphone_id is None)
    let default_checked = selected_id.is_none();
    let default_item = CheckMenuItem::with_id(
        app,
        "mic_default",
        "System Default",
        true,
        default_checked,
        None::<&str>,
    )?;
    mic_items.push(default_item);

    // Add each device
    for device in devices {
        let item_id = format!("mic_{}", device.id.replace(' ', "_"));
        let is_checked = selected_id.as_ref() == Some(&device.id);
        let item = CheckMenuItem::with_id(app, &item_id, &device.name, true, is_checked, None::<&str>)?;
        mic_items.push(item);
    }

    let mic_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        mic_items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect();

    Ok(Submenu::with_items(app, "Microphone", true, &mic_refs)?)
}

fn handle_menu_event(app: &tauri::AppHandle, event_id: &str) {
    match event_id {
        "quit" => {
            tracing::info!("Quit requested from tray");
            app.exit(0);
        }
        "open" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = app.emit("navigate-to-tab", "home");
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "history" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = app.emit("navigate-to-tab", "history");
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "settings" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = app.emit("navigate-to-tab", "settings");
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
                let _ = app.emit("navigate-to-tab", "settings");
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "about" => {
            // Open main window and navigate to about tab
            if let Some(window) = app.get_webview_window("main") {
                let _ = app.emit("navigate-to-tab", "about");
                let _ = window.show();
                let _ = window.set_focus();
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

            // Save to settings and rebuild menu
            let app_clone = app.clone();
            let device_id_clone = device_id.clone();
            tauri::async_runtime::spawn(async move {
                let mut current_settings = settings::load_settings().await;
                current_settings.selected_microphone_id = device_id_clone;
                if let Err(e) = settings::save_settings(&current_settings).await {
                    tracing::error!("Failed to save mic setting: {}", e);
                }
                // Rebuild tray menu to update checkmarks
                if let Err(e) = rebuild_tray_menu(&app_clone) {
                    tracing::error!("Failed to rebuild tray menu: {}", e);
                }
            });
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
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(ActivationPolicy::Accessory);
            }

            // Initialize paths first (required by db, settings, model)
            paths::init(app)?;

            // Initialize database
            if let Err(e) = db::init_db() {
                tracing::error!("Failed to initialize database: {}", e);
            }

            // Build tray menu based on app state
            let menu = build_tray_menu(app)?;

            // Create tray icon with explicit ID for later access
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .menu(&menu)
                .on_menu_event(|app, event| {
                    handle_menu_event(app, event.id.as_ref());
                })
                .build(app)?;

            // Prevent main window from being destroyed on close - hide it instead
            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_clone.hide();
                    }
                });
            }

            // Check if onboarding was previously completed
            let app_handle = app.handle().clone();
            let saved_settings = tauri::async_runtime::block_on(settings::load_settings());

            if saved_settings.onboarding_completed {
                // User already completed onboarding - verify model and init
                let model_path = model::default_model_path();
                let verification = model::verify(&model_path);

                if verification.is_valid {
                    // Initialize transcriber
                    let model_path_str = model_path.to_string_lossy().to_string();
                    if let Err(e) = transcribe::init_transcriber(&model_path_str) {
                        tracing::error!("Failed to init transcriber: {:?}", e);
                    } else {
                        // Set hotkey from settings before registering
                        if let Err(e) = platform::set_hotkey(&saved_settings.hotkey) {
                            tracing::warn!("Failed to set hotkey from settings: {}", e);
                        }
                        // Register hotkey (don't block Ready state if this fails)
                        if let Err(e) = hotkey::register_hotkey(&app_handle) {
                            tracing::warn!("Failed to register hotkey: {} - will work after restart with permissions", e);
                        }
                        // Transition to Ready and rebuild tray menu
                        state::set_state(&app_handle, AppState::Ready).ok();
                        if let Err(e) = rebuild_tray_menu(&app_handle) {
                            tracing::error!("Failed to rebuild tray menu: {}", e);
                        }
                        tracing::info!("Restored to Ready state from saved settings");
                    }
                } else {
                    tracing::warn!("Onboarding completed but model invalid, showing setup");
                    // Show main window for re-setup
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                    }
                }
            } else {
                // First run - show main window for onboarding
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
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
            refresh_audio_devices,
            set_audio_device,
            test_microphone,
            start_mic_test,
            get_mic_test_level,
            stop_mic_test,
            // Hotkey testing
            hotkey::test_transcription,
            hotkey::enable_onboarding_test_mode,
            hotkey::disable_onboarding_test_mode,
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
            check_model_exists,
            select_model_file,
            // Setup
            complete_setup,
            // Updates
            updates::check_for_updates_cmd,
            // Auto-start
            set_auto_start,
            get_auto_start,
            // Permissions
            check_accessibility_permission,
            request_accessibility_permission,
            request_microphone_permission,
            request_permissions,
            try_register_hotkey,
            update_hotkey,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
