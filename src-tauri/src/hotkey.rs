use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::audio::AudioCapture;
use crate::db::{save_transcript, NewTranscript};
use crate::model::{model_path_for_variant, ModelVariant};
use crate::paste::paste_text;
use crate::settings::{load_settings, load_settings_sync};
use crate::sounds;
use crate::transcribe::{
    init_transcriber, is_transcriber_loaded, transcribe_audio, unload_transcriber,
};

// Maximum recording duration (2 minutes) in milliseconds
const MAX_RECORDING_DURATION_MS: u64 = 2 * 60 * 1000;
const LAZY_MODEL_IDLE_UNLOAD_SECS: u64 = 10;

// Track if key is currently held
static KEY_HELD: AtomicBool = AtomicBool::new(false);

// Recording session ID to track auto-stop timer validity
static RECORDING_SESSION_ID: AtomicU64 = AtomicU64::new(0);
static LAZY_ACTIVITY_TOKEN: AtomicU64 = AtomicU64::new(0);
static LAZY_PRELOAD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

// Onboarding test mode - when enabled, hotkey works even during NeedsSetup state
static ONBOARDING_TEST_MODE: AtomicBool = AtomicBool::new(false);

/// Whether onboarding test mode is currently active
#[cfg(target_os = "macos")]
pub(crate) fn is_onboarding_test_mode() -> bool {
    ONBOARDING_TEST_MODE.load(Ordering::SeqCst)
}

/// Check if transcription contains only blank audio markers (no actual speech)
fn is_blank_audio(text: &str) -> bool {
    let normalized = text.to_lowercase();
    matches!(
        normalized.as_str(),
        "[blank_audio]"
            | "(blank audio)"
            | "[silence]"
            | "(silence)"
            | "[no speech]"
            | "(no speech)"
            | "[inaudible]"
    )
}

// Commands sent to the audio thread
enum AudioCommand {
    StartRecording(Option<String>),
    StopRecording,
}

// Response from the audio thread
struct AudioResponse {
    buffer: Vec<f32>,
}

// Global audio thread handle
struct AudioThread {
    cmd_tx: Sender<AudioCommand>,
    resp_rx: Receiver<AudioResponse>,
}

static AUDIO_THREAD: Mutex<Option<AudioThread>> = Mutex::new(None);

// Recording start time for duration tracking
static RECORDING_START: Mutex<Option<Instant>> = Mutex::new(None);

// Frontmost app when recording started (to restore focus before paste)
#[cfg(target_os = "macos")]
static FRONTMOST_APP: Mutex<Option<String>> = Mutex::new(None);

/// Initialize the audio thread (call once at startup or before first recording)
fn ensure_audio_thread() {
    let mut thread_guard = match AUDIO_THREAD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Audio thread mutex poisoned in ensure_audio_thread, recovering");
            poisoned.into_inner()
        }
    };
    if thread_guard.is_some() {
        return;
    }

    let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>();
    let (resp_tx, resp_rx) = mpsc::channel::<AudioResponse>();

    std::thread::spawn(move || {
        let mut capture = AudioCapture::new();

        loop {
            match cmd_rx.recv() {
                Ok(AudioCommand::StartRecording(device_id)) => {
                    capture.set_device(device_id);
                    // Initialize capture if not already
                    if let Err(e) = capture.init_capture() {
                        tracing::error!("Failed to init audio capture: {}", e);
                        // Send empty response on error
                        let _ = resp_tx.send(AudioResponse { buffer: Vec::new() });
                        continue;
                    }
                    capture.begin_recording();
                    tracing::info!("Audio recording started");
                }
                Ok(AudioCommand::StopRecording) => {
                    let buffer = capture.end_recording();
                    let resampled = capture.resample_to_16k(buffer);
                    capture.close_capture();

                    tracing::info!(
                        "Audio recording stopped, buffer size: {} samples",
                        resampled.len()
                    );

                    let _ = resp_tx.send(AudioResponse { buffer: resampled });
                }
                Err(_) => {
                    tracing::info!("Audio thread shutting down");
                    capture.close_capture();
                    break;
                }
            }
        }
    });

    *thread_guard = Some(AudioThread { cmd_tx, resp_rx });
}

fn mark_lazy_activity() -> u64 {
    LAZY_ACTIVITY_TOKEN.fetch_add(1, Ordering::SeqCst) + 1
}

async fn init_transcriber_async(model_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || init_transcriber(&model_path))
        .await
        .map_err(|e| format!("Transcriber initialization task failed: {e}"))?
        .map_err(|e| e.to_string())
}

fn spawn_lazy_preload_if_needed(variant: ModelVariant) {
    if is_transcriber_loaded() {
        return;
    }
    if LAZY_PRELOAD_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let model_path = model_path_for_variant(variant);
        if !model_path.exists() {
            tracing::warn!("Skipping lazy preload, model not found at {:?}", model_path);
            LAZY_PRELOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
            return;
        }

        let model_path_str = model_path.to_string_lossy().to_string();
        match init_transcriber_async(model_path_str).await {
            Ok(_) => tracing::info!("Lazy preload completed"),
            Err(err) => tracing::warn!("Lazy preload failed: {}", err),
        }

        LAZY_PRELOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

fn schedule_lazy_unload_if_idle() {
    let token = mark_lazy_activity();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(LAZY_MODEL_IDLE_UNLOAD_SECS)).await;

        if LAZY_ACTIVITY_TOKEN.load(Ordering::SeqCst) != token {
            return;
        }
        if KEY_HELD.load(Ordering::SeqCst) {
            return;
        }
        if crate::state::get_state() != crate::state::AppState::Ready {
            return;
        }

        let settings = load_settings().await;
        if !settings.lazy_model_loading {
            return;
        }

        unload_transcriber();
        tracing::info!(
            "Unloaded transcriber after {} seconds of idle time",
            LAZY_MODEL_IDLE_UNLOAD_SECS
        );
    });
}

/// Called when F8 is pressed down
pub fn on_key_down(app: &AppHandle) {
    let is_test_mode = ONBOARDING_TEST_MODE.load(Ordering::SeqCst);
    let settings_snapshot = load_settings_sync();

    // Check current state - only proceed if Ready (or in test mode)
    if !is_test_mode {
        let state = crate::state::get_state();
        if !state.can_record() {
            return;
        }
    }

    if !is_test_mode && settings_snapshot.lazy_model_loading {
        // Invalidate any pending lazy unload timer from older activity.
        mark_lazy_activity();
        spawn_lazy_preload_if_needed(settings_snapshot.active_model_variant);
    }

    // Capture frontmost app on a background thread to avoid delaying the UI.
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(|| {
            if let Some(bundle_id) = crate::platform::get_frontmost_app() {
                let mut frontmost_app = match FRONTMOST_APP.lock() {
                    Ok(app) => app,
                    Err(poisoned) => {
                        tracing::warn!("Frontmost app mutex poisoned on capture, recovering");
                        poisoned.into_inner()
                    }
                };
                *frontmost_app = Some(bundle_id);
            }
        });
    }

    // Transition to Recording (skip state transition in test mode to avoid triggering main.ts)
    if !is_test_mode {
        crate::state::transition_to(crate::state::AppState::Recording).ok();
        app.emit("app-state-changed", "recording").ok();
    }
    KEY_HELD.store(true, Ordering::SeqCst);

    // Show recording indicator
    crate::indicator::show_recording(app).ok();

    // Store recording start time
    {
        let mut start = match RECORDING_START.lock() {
            Ok(start) => start,
            Err(poisoned) => {
                tracing::warn!("Recording start mutex poisoned on key down, recovering");
                poisoned.into_inner()
            }
        };
        *start = Some(Instant::now());
    }

    // Increment session ID and start auto-stop timer (2 min max)
    let session_id = RECORDING_SESSION_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let app_for_timer = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(MAX_RECORDING_DURATION_MS)).await;

        // Only trigger auto-stop if this session is still active
        let current_session = RECORDING_SESSION_ID.load(Ordering::SeqCst);
        if current_session == session_id && KEY_HELD.load(Ordering::SeqCst) {
            tracing::info!("Auto-stopping recording after 2 minutes");
            crate::notifications::show_info(
                &app_for_timer,
                "Recording Stopped",
                "Maximum recording duration (2 min) reached",
            );
            on_key_up(&app_for_timer);
        }
    });

    // Ensure audio thread is running and start recording
    ensure_audio_thread();

    let thread_guard = match AUDIO_THREAD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Audio thread mutex poisoned on key down, recovering");
            poisoned.into_inner()
        }
    };
    if let Some(ref thread) = *thread_guard {
        let selected_device_id = settings_snapshot.selected_microphone_id;
        if thread
            .cmd_tx
            .send(AudioCommand::StartRecording(selected_device_id))
            .is_err()
        {
            tracing::error!("Failed to send StartRecording command");
        }
    }

    // Play start sound if enabled
    tauri::async_runtime::spawn(async move {
        let settings = load_settings().await;
        if settings.sound_enabled {
            sounds::play_start();
        }
    });
}

/// Called when F8 is released
pub fn on_key_up(app: &AppHandle) {
    let is_test_mode = ONBOARDING_TEST_MODE.load(Ordering::SeqCst);

    if !KEY_HELD.load(Ordering::SeqCst) {
        return;
    }
    KEY_HELD.store(false, Ordering::SeqCst);

    // Check we're in Recording state (skip in test mode)
    if !is_test_mode {
        let state = crate::state::get_state();
        if !matches!(state, crate::state::AppState::Recording) {
            return;
        }
    }

    // Calculate recording duration
    let duration_ms = {
        let start = match RECORDING_START.lock() {
            Ok(start) => start,
            Err(poisoned) => {
                tracing::warn!("Recording start mutex poisoned on key up, recovering");
                poisoned.into_inner()
            }
        };
        start.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0)
    };

    // Transition to Processing (skip state events in test mode)
    if !is_test_mode {
        crate::state::transition_to(crate::state::AppState::Processing).ok();
        app.emit("app-state-changed", "processing").ok();
    }

    // Show processing indicator
    crate::indicator::show_processing(app).ok();

    let duration_ms = duration_ms as i64;

    // Stop recording and get audio buffer
    // We need to send command and then receive response
    let cmd_sent = {
        let thread_guard = match AUDIO_THREAD.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("Audio thread mutex poisoned on stop, recovering");
                poisoned.into_inner()
            }
        };
        if let Some(ref thread) = *thread_guard {
            thread.cmd_tx.send(AudioCommand::StopRecording).is_ok()
        } else {
            false
        }
    };

    // Spawn async task for transcription
    let app_handle = app.clone();
    let test_mode = is_test_mode;
    tauri::async_runtime::spawn(async move {
        // Wait for audio response (blocking recv in async context via spawn_blocking)
        let audio_buffer = if cmd_sent {
            tokio::task::spawn_blocking(|| {
                let thread_guard = match AUDIO_THREAD.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        tracing::warn!("Audio thread mutex poisoned in recv task, recovering");
                        poisoned.into_inner()
                    }
                };
                if let Some(ref thread) = *thread_guard {
                    thread.resp_rx.recv().ok().map(|r| r.buffer)
                } else {
                    None
                }
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };

        // Get the audio buffer
        let audio_buffer = match audio_buffer {
            Some(buf) => buf,
            None => {
                tracing::error!("No audio buffer received");
                finish_transcription(&app_handle, None, duration_ms, test_mode).await;
                return;
            }
        };

        if audio_buffer.is_empty() {
            tracing::warn!("Empty audio buffer");
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
            return;
        }

        tracing::info!(
            "Processing {} samples ({:.1}s of audio)",
            audio_buffer.len(),
            audio_buffer.len() as f32 / 16000.0
        );

        // Load settings for model variant and language
        let settings = load_settings().await;

        // Initialize transcriber if needed
        if !is_transcriber_loaded() {
            let model_path = model_path_for_variant(settings.active_model_variant);
            let model_path_str = model_path.to_string_lossy().to_string();

            // Check if model exists
            if !model_path.exists() {
                tracing::error!("Model not found at {:?}", model_path);
                crate::notifications::show_error(
                    &app_handle,
                    "Model Not Found",
                    "Please download the model in settings",
                );
                finish_transcription(&app_handle, None, duration_ms, test_mode).await;
                return;
            }

            if let Err(e) = init_transcriber_async(model_path_str).await {
                tracing::error!("Failed to initialize transcriber: {}", e);
                crate::notifications::show_error(
                    &app_handle,
                    "Model Error",
                    &format!("Failed to load model: {e}"),
                );
                finish_transcription(&app_handle, None, duration_ms, test_mode).await;
                return;
            }
            tracing::info!("Transcriber initialized from {:?}", model_path);
        }

        // Determine language from settings
        // 1 language = use it, 2+ = auto-detect (None)
        let lang: Option<String> = if settings.languages.len() == 1 {
            Some(settings.languages[0].clone())
        } else {
            None
        };

        let dictionary_terms = crate::dictionary::sanitize_terms(&settings.dictionary_terms);
        let dictionary_prompt = crate::dictionary::build_prompt(&dictionary_terms);

        // Transcribe
        let text =
            match transcribe_audio(&audio_buffer, lang.as_deref(), dictionary_prompt.as_deref()) {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("Transcription failed: {}", e);
                    crate::notifications::show_error(
                        &app_handle,
                        "Transcription Error",
                        &format!("{e}"),
                    );
                    finish_transcription(&app_handle, None, duration_ms, test_mode).await;
                    return;
                }
            };

        // Apply user dictionary corrections and normalize whitespace edges.
        let text = crate::dictionary::apply_dictionary_corrections(text.trim(), &dictionary_terms)
            .trim()
            .to_string();

        // Filter out whisper special tokens indicating no speech
        if text.is_empty() || is_blank_audio(&text) {
            tracing::info!("No speech detected, skipping paste/save");
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
            return;
        }

        let mut persisted_transcript: Option<String> = None;

        if test_mode {
            // In test mode, emit event instead of pasting (indicator steals focus)
            app_handle
                .emit("test-transcription-result", text.clone())
                .ok();
        } else {
            // Restore focus to the app that was active when recording started
            #[cfg(target_os = "macos")]
            {
                let frontmost_app = match FRONTMOST_APP.lock() {
                    Ok(app) => app,
                    Err(poisoned) => {
                        tracing::warn!("Frontmost app mutex poisoned on restore, recovering");
                        poisoned.into_inner()
                    }
                }
                .take();
                if let Some(bundle_id) = frontmost_app {
                    // Small delay for macOS to settle
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    crate::platform::activate_app(&bundle_id).ok();
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }

            // Paste text directly (no clipboard), with trailing space for continuation
            if settings.paste_enabled {
                let paste_result = paste_text(&format!("{text} "));
                if paste_result.should_notify() {
                    tracing::warn!("Direct text input failed after transcription");
                }
            }

            // Save to history if enabled
            if settings.history_mode == crate::settings::HistoryMode::ThirtyDays {
                let transcript = NewTranscript {
                    text: text.clone(),
                    duration_ms,
                    app_context: None, // TODO: Get focused app context
                };
                match save_transcript(&transcript) {
                    Ok(_) => persisted_transcript = Some(text.clone()),
                    Err(e) => tracing::error!("Failed to save transcript: {}", e),
                }
            }
        }

        finish_transcription(&app_handle, persisted_transcript, duration_ms, test_mode).await;
    });
}

async fn finish_transcription(
    app: &AppHandle,
    text: Option<String>,
    _duration_ms: i64,
    is_test_mode: bool,
) {
    // Hide indicator
    crate::indicator::hide(app).ok();

    // Return to Ready (skip state events in test mode)
    if !is_test_mode {
        crate::state::transition_to(crate::state::AppState::Ready).ok();
        app.emit("app-state-changed", "ready").ok();
    }

    if let Some(t) = text {
        app.emit("transcript-added", t).ok();
    }

    if !is_test_mode {
        let settings = load_settings().await;
        if settings.lazy_model_loading {
            schedule_lazy_unload_if_idle();
        }
    }
}

/// Register the global hotkey listener
pub fn register_hotkey(app: &AppHandle) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        crate::hotkey_listener::start_hotkey_listener(app.clone())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        tracing::warn!("Global hotkey not implemented for this platform");
        let _ = app;
        Ok(())
    }
}

// Tauri commands for testing
#[tauri::command]
pub fn test_transcription(app: AppHandle) -> Result<(), String> {
    // Simulate a complete recording cycle for testing
    tracing::info!("Test transcription triggered");

    // Simulate key down
    on_key_down(&app);

    // Short delay then key up (simulate short recording)
    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2000)); // 2 seconds of recording
        on_key_up(&app_clone);
    });

    Ok(())
}

#[tauri::command]
pub fn enable_onboarding_test_mode(app: AppHandle) -> Result<(), String> {
    tracing::info!("Enabling onboarding test mode");
    ONBOARDING_TEST_MODE.store(true, Ordering::SeqCst);

    // Set the hotkey from settings
    let settings = load_settings_sync();
    crate::hotkey_config::set_hotkey_from_string(&settings.hotkey)?;

    // Register the hotkey listener (idempotent)
    register_hotkey(&app)?;

    Ok(())
}

#[tauri::command]
pub fn disable_onboarding_test_mode() -> Result<(), String> {
    tracing::info!("Disabling onboarding test mode");
    ONBOARDING_TEST_MODE.store(false, Ordering::SeqCst);
    Ok(())
}
