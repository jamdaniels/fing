// Fing - Fast, private, local speech-to-text

mod app_info;
mod audio;
mod db;
mod engine;
mod hotkey;
mod hotkey_config;
mod hotkey_listener;
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
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    ActivationPolicy, Emitter, Manager,
};

/// Consolidated mic test state to prevent race conditions
/// All state changes go through a single lock acquisition
#[derive(Default)]
struct MicTestState {
    running: bool,
    generation: u64,      // Incremented each time a new test starts
    level: u32,           // Fixed-point (level * 10000)
    receiving: bool,
    device_id: Option<String>,
}

#[derive(Clone)]
struct MicMenuEntry {
    device_id: Option<String>,
    item: CheckMenuItem<tauri::Wry>,
}

lazy_static::lazy_static! {
    static ref MIC_TEST_STATE: Mutex<MicTestState> = Mutex::new(MicTestState::default());
    static ref MIC_MENU_ITEMS: Mutex<Vec<MicMenuEntry>> = Mutex::new(Vec::new());
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

    // Mark as running and get generation BEFORE spawning thread
    let my_generation = {
        let mut state = MIC_TEST_STATE
            .lock()
            .map_err(|e| format!("Mic test state lock poisoned: {}", e))?;
        state.generation += 1;
        state.running = true;
        state.generation
    };

    // Start mic test thread (uses std::thread for blocking audio I/O)
    let device_id_clone = device_id.clone();
    std::thread::spawn(move || {
        let mut capture = AudioCapture::new();
        if let Some(id) = device_id_clone {
            capture.set_device(Some(id));
        }

        // Check if we should still run (stop or new test might have been started)
        let should_run = MIC_TEST_STATE
            .lock()
            .map(|s| s.running && s.generation == my_generation)
            .unwrap_or(false);
        if !should_run {
            tracing::info!("Mic test cancelled before init (gen {})", my_generation);
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
            // Check if we should stop (either stopped or a new test started)
            let should_run = MIC_TEST_STATE
                .lock()
                .map(|s| s.running && s.generation == my_generation)
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
    hotkey_config::set_hotkey_from_string(&hotkey)
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

    // Set hotkey from settings and register listener.
    if let Err(e) = hotkey_config::set_hotkey_from_string(&current_settings.hotkey) {
        tracing::warn!("Failed to parse hotkey from settings: {}", e);
    } else if let Err(e) = hotkey::register_hotkey(&app) {
        // Don't fail setup if this fails - user can fix permissions and restart
        tracing::warn!("Failed to register hotkey: {} - hotkey will work after app restart with proper permissions", e);
    }

    // Transition to Ready state (this also emits app-state-changed event)
    state::set_state(&app, AppState::Ready)?;

    // Rebuild tray menu to show full menu instead of "Complete Setup..."
    if let Err(e) = rebuild_tray_menu(&app) {
        tracing::error!("Failed to rebuild tray menu: {}", e);
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
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

        // Build microphone items (flattened into main menu)
        let mic_items = build_mic_menu_items(app)?;

        let separator2 = PredefinedMenuItem::separator(app)?;
        let updates = MenuItem::with_id(app, "check_updates", "Check for Updates", true, None::<&str>)?;
        let separator3 = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

        // Build menu with mic items spread inline
        let mut items: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = vec![
            &open, &history, &settings, &separator1,
        ];
        for item in &mic_items {
            items.push(item.as_ref());
        }
        items.extend([&separator2 as &dyn tauri::menu::IsMenuItem<tauri::Wry>, &updates, &separator3, &quit]);

        Ok(Menu::with_items(app, &items)?)
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

fn build_mic_menu_items(app: &impl tauri::Manager<tauri::Wry>) -> Result<Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>>, Box<dyn std::error::Error>> {
    let devices = AudioCapture::list_devices();
    let current_settings = settings::load_settings_sync();
    let selected_id = current_settings.selected_microphone_id;
    tracing::debug!(
        "Building mic menu items: devices={}, has_selected_id={}",
        devices.len(),
        selected_id.is_some()
    );

    let has_selected_device = selected_id
        .as_ref()
        .map(|id| devices.iter().any(|device| &device.id == id))
        .unwrap_or(false);

    let mut mic_entries: Vec<MicMenuEntry> = Vec::new();
    let mut result: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = Vec::new();

    // Add disabled "Microphone" label as section title
    let mic_label = MenuItem::with_id(app, "mic_label", "Microphone", false, None::<&str>)?;
    result.push(Box::new(mic_label));

    // Add each device - auto-select default device when no selection exists
    for device in &devices {
        let item_id = format!("mic_{}", encode_menu_id(&device.id));
        let is_checked = if has_selected_device {
            selected_id.as_ref() == Some(&device.id)
        } else {
            device.is_default
        };
        let item = CheckMenuItem::with_id(app, &item_id, &device.name, true, is_checked, None::<&str>)?;
        mic_entries.push(MicMenuEntry {
            device_id: Some(device.id.clone()),
            item: item.clone(),
        });
        result.push(Box::new(item));
    }

    let mut stored = MIC_MENU_ITEMS.lock().unwrap();
    *stored = mic_entries;

    Ok(result)
}

fn encode_menu_id(value: &str) -> String {
    value.as_bytes().iter().map(|b| format!("{:02x}", b)).collect()
}

fn decode_menu_id(value: &str) -> Option<String> {
    if value.len() % 2 != 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let mut iter = value.as_bytes().chunks(2);
    while let Some(pair) = iter.next() {
        let hex = std::str::from_utf8(pair).ok()?;
        let byte = u8::from_str_radix(hex, 16).ok()?;
        bytes.push(byte);
    }

    String::from_utf8(bytes).ok()
}

fn update_mic_menu_checks(selected_id: Option<String>) {
    let stored = match MIC_MENU_ITEMS.lock() {
        Ok(items) => items,
        Err(_) => return,
    };

    let has_selected_device = selected_id
        .as_ref()
        .map(|id| stored.iter().any(|entry| entry.device_id.as_ref() == Some(id)))
        .unwrap_or(false);

    for entry in stored.iter() {
        let checked = match (&entry.device_id, &selected_id) {
            (Some(device_id), Some(selected)) => device_id == selected,
            (Some(_), None) => false, // Will need to check is_default, but we don't have that info here
            _ => false,
        };
        // When no selection, we can't easily determine default here, so just uncheck all
        // The menu will be rebuilt with correct state on next app launch
        let _ = entry.item.set_checked(if has_selected_device { checked } else { false });
    }
}

fn handle_menu_event(app: &tauri::AppHandle, event_id: &str) {
    fn show_window_for_tab(app: &tauri::AppHandle, tab: &str) {
        if let Some(window) = app.get_webview_window("main") {
            let tab_json = serde_json::to_string(tab).unwrap_or_else(|_| "\"home\"".to_string());
            let script = format!("window.__navigateTo && window.__navigateTo({});", tab_json);
            let _ = window.eval(&script);
            let _ = window.show();
            let _ = window.set_focus();
        }
    }

    match event_id {
        "quit" => {
            tracing::info!("Quit requested from tray");
            app.exit(0);
        }
        "open" => {
            show_window_for_tab(app, "home");
        }
        "history" => {
            show_window_for_tab(app, "history");
        }
        "settings" => {
            show_window_for_tab(app, "settings");
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
            show_window_for_tab(app, "settings");
        }
        id if id.starts_with("mic_") => {
            tracing::debug!("Mic menu event: {}", id);
            let encoded = id.strip_prefix("mic_").unwrap_or("");
            let device_id = match decode_menu_id(encoded) {
                Some(decoded) => Some(decoded),
                None => {
                    tracing::warn!("Invalid microphone menu id: {}", id);
                    return;
                }
            };
            let current_selected = settings::load_settings_sync().selected_microphone_id;
            if device_id == current_selected {
                return;
            }
            tracing::info!("Microphone changed via tray: {:?}", device_id);

            // Save to settings and update checkmarks
            let device_id_clone = device_id.clone();
            tauri::async_runtime::spawn(async move {
                let mut current_settings = settings::load_settings().await;
                current_settings.selected_microphone_id = device_id_clone;
                if let Err(e) = settings::save_settings(&current_settings).await {
                    tracing::error!("Failed to save mic setting: {}", e);
                }
                update_mic_menu_checks(current_settings.selected_microphone_id.clone());
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

            // Check if onboarding was previously completed
            let app_handle = app.handle().clone();
            let saved_settings = tauri::async_runtime::block_on(settings::load_settings());
            let mut show_setup_window = false;

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
                        if let Err(e) = hotkey_config::set_hotkey_from_string(&saved_settings.hotkey) {
                            tracing::warn!("Failed to parse hotkey from settings: {}", e);
                        } else if let Err(e) = hotkey::register_hotkey(&app_handle) {
                            // Don't block Ready state if this fails
                            tracing::warn!("Failed to register hotkey: {} - will work after restart with permissions", e);
                        }
                        // Transition to Ready before building tray menu
                        state::set_state(&app_handle, AppState::Ready).ok();
                        tracing::info!("Restored to Ready state from saved settings");
                    }
                } else {
                    tracing::warn!("Onboarding completed but model invalid, showing setup");
                    show_setup_window = true;
                }
            } else {
                show_setup_window = true;
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

            if show_setup_window {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
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
