// Fing - Fast, private, local speech-to-text

mod app_info;
mod audio;
mod db;
mod dictionary;
mod engine;
mod hotkey;
mod hotkey_config;
mod hotkey_listener;
mod i18n;
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
mod update;

use audio::{AudioCapture, AudioDevice, MicrophoneTest};
use state::AppState;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Mutex, OnceLock,
};
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

/// Consolidated mic test state to prevent race conditions
/// All state changes go through a single lock acquisition
#[derive(Default)]
struct MicTestState {
    running: bool,
    generation: u64, // Incremented each time a new test starts
    level: u32,      // Fixed-point (level * 10000)
    receiving: bool,
    device_id: Option<String>,
}

lazy_static::lazy_static! {
    static ref MIC_TEST_STATE: Mutex<MicTestState> = Mutex::new(MicTestState::default());
}

static PERMISSION_RESTART_ARMED: AtomicBool = AtomicBool::new(false);
static NEXT_MAIN_WINDOW_PRESENTATION_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
static PENDING_MAIN_WINDOW_PRESENTATION_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

const MAIN_WINDOW_PRESENTATION_FALLBACK_MS: u64 = 250;

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MicTestStartResult {
    pub requested_device: Option<String>,
    pub actual_device: String,
    pub device_matched: bool,
}

enum FrontendHotkeyCommand {
    Press(tauri::AppHandle),
    Release(tauri::AppHandle),
}

static FRONTEND_HOTKEY_TX: OnceLock<mpsc::Sender<FrontendHotkeyCommand>> = OnceLock::new();

fn frontend_hotkey_sender() -> &'static mpsc::Sender<FrontendHotkeyCommand> {
    FRONTEND_HOTKEY_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<FrontendHotkeyCommand>();

        std::thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                match command {
                    FrontendHotkeyCommand::Press(app) => hotkey::on_key_down(&app),
                    FrontendHotkeyCommand::Release(app) => hotkey::on_key_up(&app),
                }
            }
        });

        tx
    })
}

#[tauri::command]
fn get_app_state() -> String {
    state::get_state().as_str().to_string()
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BootstrapStatus {
    app_state: String,
    should_show_onboarding: bool,
    onboarding_completed: bool,
    reason: String,
}

#[derive(Clone)]
struct BootstrapContext {
    decision: BootstrapDecision,
    settings: settings::Settings,
    available_model_path: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BootstrapDecision {
    app_state: AppState,
    should_show_onboarding: bool,
    reason: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveModelValidity {
    NotChecked,
    Valid,
    Missing,
    Invalid,
}

impl BootstrapDecision {
    fn ready() -> Self {
        Self::completed("ready")
    }

    fn completed(reason: &'static str) -> Self {
        Self {
            app_state: AppState::Ready,
            should_show_onboarding: false,
            reason,
        }
    }

    fn needs_setup(reason: &'static str) -> Self {
        Self {
            app_state: AppState::NeedsSetup,
            should_show_onboarding: true,
            reason,
        }
    }

    fn status(self, settings: &settings::Settings) -> BootstrapStatus {
        BootstrapStatus {
            app_state: self.app_state.as_str().to_string(),
            should_show_onboarding: self.should_show_onboarding,
            onboarding_completed: settings.onboarding_completed,
            reason: self.reason.to_string(),
        }
    }
}

fn resolve_bootstrap_decision(
    saved_settings: &settings::Settings,
    active_model_validity: ActiveModelValidity,
) -> BootstrapDecision {
    if !saved_settings.onboarding_completed {
        return BootstrapDecision::needs_setup("incomplete_onboarding");
    }

    match active_model_validity {
        ActiveModelValidity::Valid => BootstrapDecision::ready(),
        ActiveModelValidity::Missing => BootstrapDecision::needs_setup("model_missing"),
        ActiveModelValidity::Invalid | ActiveModelValidity::NotChecked => {
            BootstrapDecision::needs_setup("model_invalid")
        }
    }
}

async fn load_bootstrap_context(phase: &'static str) -> BootstrapContext {
    let saved_settings = settings::load_settings_uncached().await;
    let (active_model_validity, available_model_path) = if saved_settings.onboarding_completed {
        let variant = saved_settings.active_model_variant;
        let inspection_started = std::time::Instant::now();
        match tauri::async_runtime::spawn_blocking(move || {
            let path = model::model_path_for_variant(variant);
            let inspection = model::inspect_for_variant(&path, variant);
            (path, inspection)
        })
        .await
        {
            Ok((path, inspection)) if inspection.is_valid => {
                tracing::info!(
                    "Inspected {:?} model in {} ms",
                    variant,
                    inspection_started.elapsed().as_millis()
                );
                (ActiveModelValidity::Valid, Some(path))
            }
            Ok((_, inspection)) if !inspection.exists => {
                tracing::warn!("Active {:?} model is missing", variant);
                (ActiveModelValidity::Missing, None)
            }
            Ok(_) => {
                tracing::warn!("Active {:?} model failed structural inspection", variant);
                (ActiveModelValidity::Invalid, None)
            }
            Err(_) => {
                tracing::warn!("Active model inspection task failed");
                (ActiveModelValidity::NotChecked, None)
            }
        }
    } else {
        (ActiveModelValidity::NotChecked, None)
    };
    let decision = resolve_bootstrap_decision(&saved_settings, active_model_validity);

    tracing::info!(
        "Bootstrap decision: phase={}, reason={}, onboarding_completed={}, model_validity={:?}, should_show_onboarding={}, app_state={}",
        phase,
        decision.reason,
        saved_settings.onboarding_completed,
        active_model_validity,
        decision.should_show_onboarding,
        decision.app_state.as_str(),
    );

    BootstrapContext {
        decision,
        settings: saved_settings,
        available_model_path,
    }
}

#[tauri::command]
async fn get_bootstrap_status() -> Result<BootstrapStatus, String> {
    let context = load_bootstrap_context("ipc").await;
    Ok(context.decision.status(&context.settings))
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
async fn start_mic_test(device_id: Option<String>) -> Result<MicTestStartResult, String> {
    tracing::info!("Starting mic test with device: {:?}", device_id);

    // Stop any existing test and reset state atomically
    {
        let mut state = MIC_TEST_STATE
            .lock()
            .map_err(|e| format!("Mic test state lock poisoned: {e}"))?;
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
        tracing::warn!("Device mismatch! Requested device not found, using fallback.");
    }

    // Mark as running and get generation BEFORE spawning thread
    let my_generation = {
        let mut state = MIC_TEST_STATE
            .lock()
            .map_err(|e| format!("Mic test state lock poisoned: {e}"))?;
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
async fn download_model(variant: model::ModelVariant) -> Result<String, String> {
    model::download_variant(variant)
        .await
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn get_download_progress() -> model::DownloadProgress {
    model::get_progress()
}

#[tauri::command]
fn get_models() -> Vec<model::ModelInfo> {
    let settings = settings::load_settings_sync();
    model::get_all_models(settings.active_model_variant)
}

#[tauri::command]
fn delete_model(variant: model::ModelVariant) -> Result<(), String> {
    // Check if this is the active model
    let settings = settings::load_settings_sync();
    if settings.active_model_variant == variant {
        return Err("Cannot delete the active model".to_string());
    }
    model::delete_model(variant)
}

#[tauri::command]
async fn set_active_model(
    variant: model::ModelVariant,
    _app: tauri::AppHandle,
) -> Result<bool, String> {
    model::ensure_variant_verified(variant)?;

    // Get current settings
    let mut current_settings = settings::load_settings().await;
    let needs_restart = current_settings.active_model_variant != variant;

    // Update settings
    current_settings.active_model_variant = variant;
    settings::save_settings(&current_settings).await?;

    // If app is in Ready state, we need to restart to reload the model
    let current_state = state::get_state();
    if needs_restart && current_state == AppState::Ready {
        tracing::info!("Model variant changed to {:?}, restart required", variant);
        // Return true to indicate restart is required
        return Ok(true);
    }

    // If in NeedsSetup state, no restart needed - model will be loaded on complete_setup
    Ok(false)
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    tracing::info!("Application shutdown requested");
    app.exit(0);
}

#[tauri::command]
fn present_main_window(app: tauri::AppHandle, frontmost: bool) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_always_on_top(frontmost);

        if frontmost {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();

            #[cfg(target_os = "macos")]
            platform::activate_current_app();
        }
    }
}

fn show_main_window_now(app: &tauri::AppHandle) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Err("Main window not found".to_string());
    };

    let _ = window.unminimize();
    window
        .show()
        .map_err(|e| format!("Failed to show main window: {e}"))?;
    let _ = window.set_focus();

    #[cfg(target_os = "macos")]
    platform::activate_current_app();

    Ok(())
}

#[tauri::command]
fn finish_main_window_presentation(app: tauri::AppHandle, request_id: u64) -> Result<(), String> {
    if PENDING_MAIN_WINDOW_PRESENTATION_REQUEST_ID
        .compare_exchange(request_id, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }

    show_main_window_now(&app)
}

#[tauri::command]
fn arm_permission_restart() {
    PERMISSION_RESTART_ARMED.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn set_auto_start(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable().map_err(|e| e.to_string())
    } else {
        autostart.disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn get_auto_start(app: tauri::AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
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
async fn update_settings(
    app: tauri::AppHandle,
    settings: settings::Settings,
) -> Result<settings::Settings, String> {
    let previous_settings = settings::load_settings_sync();
    let previous_lazy_mode = previous_settings.lazy_model_loading;
    let previous_ui_language = previous_settings.ui_language;
    let updated = settings::update_settings(settings).await?;

    if previous_lazy_mode != updated.lazy_model_loading {
        if updated.lazy_model_loading {
            transcribe::unload_transcriber();
            tracing::info!("Lazy model loading enabled, transcriber unloaded");
        } else {
            let model_path = match model::ensure_variant_available(updated.active_model_variant) {
                Ok(path) => path,
                Err(load_err) => {
                    let mut rollback = updated.clone();
                    rollback.lazy_model_loading = true;
                    if let Err(save_err) = settings::save_settings(&rollback).await {
                        tracing::error!(
                                "Failed to roll back lazy model loading setting after invalid model: {}",
                                save_err
                            );
                    }
                    return Err(load_err);
                }
            };

            let model_path_str = model_path.to_string_lossy().to_string();
            let load_result = tauri::async_runtime::spawn_blocking(move || {
                transcribe::init_transcriber(&model_path_str)
            })
            .await
            .map_err(|e| format!("Transcriber initialization task failed: {e}"))?;

            if let Err(load_err) = load_result {
                let mut rollback = updated.clone();
                rollback.lazy_model_loading = true;
                if let Err(save_err) = settings::save_settings(&rollback).await {
                    tracing::error!(
                        "Failed to roll back lazy model loading setting after load error: {}",
                        save_err
                    );
                }
                return Err(format!("Failed to load model: {load_err}"));
            }
        }
    }

    if previous_ui_language != updated.ui_language {
        if let Err(error) = rebuild_tray_menu(&app) {
            tracing::warn!("Failed to rebuild tray menu after language change: {error}");
        }
        if let Err(error) = app.emit(
            "ui-language-changed",
            serde_json::json!({ "language": updated.ui_language.code() }),
        ) {
            tracing::warn!("Failed to emit UI language change: {error}");
        }
    }

    Ok(updated)
}

#[tauri::command]
fn request_microphone_permission() {
    #[cfg(target_os = "macos")]
    platform::request_microphone_permission();
}

#[tauri::command]
fn update_hotkey(hotkey: String) -> Result<(), String> {
    hotkey_config::set_hotkey_from_string(&hotkey)
}

#[tauri::command]
fn set_hotkey_suppressed(suppressed: bool) {
    hotkey_listener::set_suppressed(suppressed);
}

fn apply_saved_hotkey(app: &tauri::AppHandle, hotkey: &str, warning: &str) {
    if let Err(e) = hotkey_config::set_hotkey_from_string(hotkey) {
        tracing::warn!(
            "{}: {} - hotkey listener will remain inactive until the user sets a new hotkey",
            warning,
            e
        );
        if let Err(clear_error) = hotkey_config::clear_hotkey_config() {
            tracing::warn!("Failed to clear invalid hotkey config: {}", clear_error);
        }
    }

    if let Err(e) = hotkey::register_hotkey(app) {
        tracing::warn!(
            "Failed to register hotkey: {} - hotkey will work after app restart with proper permissions",
            e
        );
    }
}

// Frontend hotkey handling for Windows WebView2 workaround
// WebView2 doesn't properly propagate keyboard events to WH_KEYBOARD_LL hooks
// so we need to handle hotkeys from JavaScript when the window is focused
#[tauri::command]
fn hotkey_press(app: tauri::AppHandle) {
    if let Err(e) = frontend_hotkey_sender().send(FrontendHotkeyCommand::Press(app.clone())) {
        tracing::warn!("Frontend hotkey press worker unavailable: {}", e);
        hotkey::on_key_down(&app);
    }
}

#[tauri::command]
fn hotkey_release(app: tauri::AppHandle) {
    if let Err(e) = frontend_hotkey_sender().send(FrontendHotkeyCommand::Release(app.clone())) {
        tracing::warn!("Frontend hotkey release worker unavailable: {}", e);
        hotkey::on_key_up(&app);
    }
}

#[tauri::command]
async fn complete_setup(app: tauri::AppHandle) -> Result<(), String> {
    // Load settings to get active model variant
    let current_settings = settings::load_settings().await;
    let variant = current_settings.active_model_variant;

    let model_path = model::ensure_variant_verified(variant)?;

    if !current_settings.lazy_model_loading {
        let model_path_str = model_path.to_string_lossy().to_string();
        let init_result = tauri::async_runtime::spawn_blocking(move || {
            transcribe::init_transcriber(&model_path_str)
        })
        .await
        .map_err(|e| format!("Transcriber initialization task failed: {e}"))?;

        init_result.map_err(|e| format!("Failed to initialize transcriber: {e}"))?;
    }

    settings::update_settings_atomic(|latest_settings| {
        latest_settings.onboarding_completed = true;
    })
    .await?;

    // Set hotkey from settings and register listener.
    // Don't fail setup if this fails - user can fix permissions and restart
    apply_saved_hotkey(
        &app,
        &current_settings.hotkey,
        "Failed to parse hotkey from settings",
    );

    // Transition to Ready state (this also emits app-state-changed event)
    state::set_state(&app, AppState::Ready)?;

    // Rebuild tray menu to show full menu instead of "Complete Setup..."
    if let Err(e) = rebuild_tray_menu(&app) {
        tracing::error!("Failed to rebuild tray menu: {}", e);
    }

    if let Err(error) = update::enable_after_onboarding(&app).await {
        tracing::warn!("Post-setup update initialization failed: {}", error);
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }

    tracing::info!("Setup complete with {:?} model, app is ready", variant);
    Ok(())
}

fn build_tray_menu(app: &tauri::App) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let current_state = state::get_state();
    build_tray_menu_for_state(app, current_state)
}

fn build_tray_menu_for_state(
    app: &impl tauri::Manager<tauri::Wry>,
    current_state: AppState,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let translations = i18n::current();
    if current_state == AppState::NeedsSetup {
        // Simplified menu for setup state
        let setup = MenuItem::with_id(
            app,
            "setup",
            &translations.tray.complete_setup,
            true,
            None::<&str>,
        )?;
        let separator = PredefinedMenuItem::separator(app)?;
        let quit = MenuItem::with_id(app, "quit", &translations.tray.quit, true, None::<&str>)?;

        Ok(Menu::with_items(app, &[&setup, &separator, &quit])?)
    } else {
        // Full menu for normal operation
        let open = MenuItem::with_id(app, "open", &translations.tray.open_app, true, None::<&str>)?;
        let history = MenuItem::with_id(
            app,
            "history",
            &translations.tray.history,
            true,
            None::<&str>,
        )?;
        let settings = MenuItem::with_id(
            app,
            "settings",
            &translations.tray.settings,
            true,
            None::<&str>,
        )?;
        let separator1 = PredefinedMenuItem::separator(app)?;
        let separator2 = PredefinedMenuItem::separator(app)?;
        let updates = MenuItem::with_id(
            app,
            "check_updates",
            update::tray_menu_label(),
            true,
            None::<&str>,
        )?;
        let quit = MenuItem::with_id(app, "quit", &translations.tray.quit, true, None::<&str>)?;
        Ok(Menu::with_items(
            app,
            &[
                &open,
                &history,
                &settings,
                &separator1,
                &updates,
                &separator2,
                &quit,
            ],
        )?)
    }
}

/// Tray icon ID constant
const TRAY_ID: &str = "fing-tray";

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MainWindowPresentationRequest {
    request_id: u64,
    tab: String,
}

/// Rebuild the tray menu based on current app state
pub(crate) fn rebuild_tray_menu(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
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

fn show_pending_main_window_request(app: &tauri::AppHandle, request_id: u64, source: &str) {
    if PENDING_MAIN_WINDOW_PRESENTATION_REQUEST_ID
        .compare_exchange(request_id, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    if let Err(error) = show_main_window_now(app) {
        tracing::warn!("Failed to show main window after {source}: {error}");
    }
}

fn present_main_window_for_tab(app: &tauri::AppHandle, tab: &str) {
    let request_id = NEXT_MAIN_WINDOW_PRESENTATION_REQUEST_ID.fetch_add(1, Ordering::Relaxed) + 1;
    PENDING_MAIN_WINDOW_PRESENTATION_REQUEST_ID.store(request_id, Ordering::SeqCst);

    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("Main window not found while presenting tab: {tab}");
        let _ = PENDING_MAIN_WINDOW_PRESENTATION_REQUEST_ID.compare_exchange(
            request_id,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        return;
    };

    let request = MainWindowPresentationRequest {
        request_id,
        tab: tab.to_string(),
    };

    if let Err(error) = window.emit("main-window-presentation-request", request) {
        tracing::warn!("Failed to request frontend window presentation: {error}");
        show_pending_main_window_request(app, request_id, "presentation request failure");
        return;
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(
            MAIN_WINDOW_PRESENTATION_FALLBACK_MS,
        ))
        .await;
        show_pending_main_window_request(&app_handle, request_id, "presentation timeout");
    });
}

fn handle_menu_event(app: &tauri::AppHandle, event_id: &str) {
    match event_id {
        "quit" => {
            tracing::info!("Quit requested from tray");
            app.exit(0);
        }
        "open" => {
            present_main_window_for_tab(app, "home");
        }
        "history" => {
            present_main_window_for_tab(app, "history");
        }
        "settings" => {
            present_main_window_for_tab(app, "settings");
        }
        "setup" => {
            // Open main window for onboarding
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "check_updates" => {
            // Open main window and trigger the settings update flow.
            present_main_window_for_tab(app, "settings");
            let _ = app.emit("check-for-updates", ());
        }
        _ => {
            tracing::debug!("Unknown menu event: {}", event_id);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Use try_init to avoid panic if stderr isn't available (Windows without console).
    let _ = tracing_subscriber::fmt::try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tracing::info!("Second launch detected, focusing existing window");
            present_main_window_for_tab(app, "home");
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
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

            // Prune transcripts older than 30 days
            match db::prune_old_transcripts() {
                Ok(0) => {}
                Ok(n) => tracing::info!("Pruned {n} old transcripts"),
                Err(e) => tracing::warn!("Failed to prune old transcripts: {e}"),
            }

            // Keep retention enforced for tray-app instances that remain open
            // for longer than the configured 30-day history window.
            tauri::async_runtime::spawn(async {
                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(60 * 60 * 24));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                interval.tick().await;

                loop {
                    interval.tick().await;
                    match db::prune_old_transcripts() {
                        Ok(0) => {}
                        Ok(n) => tracing::info!("Pruned {n} old transcripts"),
                        Err(e) => tracing::warn!("Failed to prune old transcripts: {e}"),
                    }
                }
            });

            // Check if onboarding was previously completed
            let app_handle = app.handle().clone();
            let bootstrap_context = tauri::async_runtime::block_on(load_bootstrap_context("setup"));
            let bootstrap_decision = bootstrap_context.decision;
            let saved_settings = bootstrap_context.settings;
            let available_model_path = bootstrap_context.available_model_path;
            let show_setup_window = bootstrap_decision.should_show_onboarding;
            let mut updates_enabled = false;

            if bootstrap_decision.app_state == AppState::Ready {
                // Try to pre-load the transcriber, but don't block Ready state
                // on failure. The hotkey handler will retry on first use.
                if !saved_settings.lazy_model_loading {
                    if let Some(model_path) = available_model_path {
                        if let Err(e) = tauri::async_runtime::block_on(async move {
                            let model_path_str = model_path.to_string_lossy().to_string();
                            tauri::async_runtime::spawn_blocking(move || {
                                transcribe::init_transcriber(&model_path_str)
                                    .map_err(|_| "transcriber initialization failed")
                            })
                            .await
                            .map_err(|_| "transcriber preload task failed")?
                        }) {
                            tracing::warn!("Transcriber init deferred to first use: {}", e);
                        }
                    } else {
                        tracing::warn!("Available model path unavailable for eager preload");
                    }
                }

                apply_saved_hotkey(
                    &app_handle,
                    &saved_settings.hotkey,
                    "Failed to parse hotkey from settings",
                );
                state::transition_to(AppState::Ready).ok();
                updates_enabled = true;
                if let Err(error) =
                    tauri::async_runtime::block_on(update::initialize_for_ready_app())
                {
                    tracing::warn!("Failed to restore persisted update state: {}", error);
                }
                tracing::info!("Restored to Ready state from saved settings");
            }

            // Build tray menu based on app state
            let menu = build_tray_menu(app)?;

            // Create tray icon with explicit ID for later access
            // macOS: white template icon that respects dark/light mode
            // Windows: colored app icon
            TrayIconBuilder::with_id(TRAY_ID)
                .icon({
                    #[cfg(target_os = "macos")]
                    {
                        tauri::include_image!("icons/tray.png")
                    }
                    #[cfg(target_os = "windows")]
                    {
                        tauri::include_image!("icons/32x32.png")
                    }
                })
                .icon_as_template(cfg!(target_os = "macos"))
                .tooltip("Fing")
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
                        let _ = window_clone.emit("main-window-hidden", ());
                    }
                });
            }

            if show_setup_window {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            if updates_enabled {
                update::schedule_startup_check(&app_handle);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            get_bootstrap_status,
            // App info
            app_info::get_app_info,
            update::get_update_status,
            // Settings
            settings::get_settings,
            update_settings,
            update::check_for_updates_now,
            update::clear_update_status,
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
            // Window management
            quit_app,
            present_main_window,
            finish_main_window_presentation,
            arm_permission_restart,
            // Model management
            download_model,
            get_download_progress,
            get_models,
            delete_model,
            set_active_model,
            // Setup
            complete_setup,
            // Auto-start
            set_auto_start,
            get_auto_start,
            // Permissions
            request_accessibility_permission,
            request_microphone_permission,
            request_permissions,
            update_hotkey,
            set_hotkey_suppressed,
            // Frontend hotkey handling (WebView2 workaround)
            hotkey_press,
            hotkey_release,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            let _ = (&app, &event);

            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::ExitRequested { code: None, .. } = event {
                if PERMISSION_RESTART_ARMED.load(Ordering::Relaxed) {
                    app.request_restart();
                }
            }
        });
}

#[cfg(test)]
mod bootstrap_tests {
    use super::*;

    fn completed_settings() -> settings::Settings {
        settings::Settings {
            onboarding_completed: true,
            ..settings::Settings::default()
        }
    }

    #[test]
    fn bootstrap_ready_when_onboarding_completed_and_model_structurally_valid() {
        let decision =
            resolve_bootstrap_decision(&completed_settings(), ActiveModelValidity::Valid);

        assert_eq!(decision.app_state, AppState::Ready);
        assert!(!decision.should_show_onboarding);
        assert_eq!(decision.reason, "ready");
    }

    #[test]
    fn bootstrap_needs_setup_when_onboarding_is_incomplete() {
        let decision = resolve_bootstrap_decision(
            &settings::Settings::default(),
            ActiveModelValidity::NotChecked,
        );

        assert_eq!(decision.app_state, AppState::NeedsSetup);
        assert!(decision.should_show_onboarding);
        assert_eq!(decision.reason, "incomplete_onboarding");
    }

    #[test]
    fn bootstrap_needs_model_repair_when_active_model_is_missing() {
        let decision =
            resolve_bootstrap_decision(&completed_settings(), ActiveModelValidity::Missing);

        assert_eq!(decision.app_state, AppState::NeedsSetup);
        assert!(decision.should_show_onboarding);
        assert_eq!(decision.reason, "model_missing");
    }

    #[test]
    fn bootstrap_needs_model_repair_when_active_model_is_invalid() {
        let decision =
            resolve_bootstrap_decision(&completed_settings(), ActiveModelValidity::Invalid);

        assert_eq!(decision.app_state, AppState::NeedsSetup);
        assert!(decision.should_show_onboarding);
        assert_eq!(decision.reason, "model_invalid");
    }
}
