use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::audio::AudioCapture;
use crate::db::{save_transcript, NewTranscript};
use crate::model::{ensure_variant_verified, model_path_for_variant, ModelVariant};
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
    StartRecording {
        device_id: Option<String>,
        reply_tx: Sender<Result<(), String>>,
    },
    StopRecording {
        reply_tx: Sender<Result<Vec<f32>, String>>,
    },
}

#[derive(Clone, Debug)]
enum RecordingStartStatus {
    Idle,
    Started { session_id: u64 },
    Failed { session_id: u64, message: String },
}

// Global audio thread handle
struct AudioThread {
    cmd_tx: Sender<AudioCommand>,
}

static AUDIO_THREAD: Mutex<Option<AudioThread>> = Mutex::new(None);
static RECORDING_START_STATUS: Mutex<RecordingStartStatus> = Mutex::new(RecordingStartStatus::Idle);

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

    std::thread::spawn(move || {
        let mut capture = AudioCapture::new();

        loop {
            match cmd_rx.recv() {
                Ok(AudioCommand::StartRecording {
                    device_id,
                    reply_tx,
                }) => {
                    capture.set_device(device_id.clone());
                    // Initialize capture if not already
                    let match_result = match capture.init_capture() {
                        Ok(result) => result,
                        Err(e) => {
                            tracing::error!("Failed to init audio capture: {}", e);
                            let _ = reply_tx.send(Err(format!("Unable to start recording: {e}")));
                            continue;
                        }
                    };

                    capture.begin_recording();
                    tracing::info!("Audio recording started");
                    let _ = reply_tx.send(Ok(()));

                    if !match_result.matched {
                        tracing::warn!(
                            "Requested microphone {:?} unavailable, recording with '{}'",
                            match_result.requested,
                            match_result.actual
                        );
                        persist_fallback_microphone_selection(device_id, match_result.actual);
                    }
                }
                Ok(AudioCommand::StopRecording { reply_tx }) => {
                    let buffer = capture.end_recording();
                    let resampled = capture.resample_to_16k(buffer);
                    capture.close_capture();

                    tracing::info!(
                        "Audio recording stopped, buffer size: {} samples",
                        resampled.len()
                    );

                    let _ = reply_tx.send(Ok(resampled));
                }
                Err(_) => {
                    tracing::info!("Audio thread shutting down");
                    capture.close_capture();
                    break;
                }
            }
        }
    });

    *thread_guard = Some(AudioThread { cmd_tx });
}

fn set_recording_start_status(status: RecordingStartStatus) {
    let mut start_status = match RECORDING_START_STATUS.lock() {
        Ok(status_guard) => status_guard,
        Err(poisoned) => {
            tracing::warn!("Recording start status mutex poisoned, recovering");
            poisoned.into_inner()
        }
    };
    *start_status = status;
}

fn current_recording_start_status() -> RecordingStartStatus {
    let start_status = match RECORDING_START_STATUS.lock() {
        Ok(status_guard) => status_guard,
        Err(poisoned) => {
            tracing::warn!("Recording start status mutex poisoned on read, recovering");
            poisoned.into_inner()
        }
    };
    start_status.clone()
}

fn send_start_recording_command(
    cmd_tx: &Sender<AudioCommand>,
    device_id: Option<String>,
) -> Result<(), String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx
        .send(AudioCommand::StartRecording {
            device_id,
            reply_tx,
        })
        .map_err(|_| "Failed to send StartRecording command".to_string())?;

    reply_rx
        .recv()
        .map_err(|_| "Audio thread dropped StartRecording response".to_string())?
}

fn send_stop_recording_command(cmd_tx: &Sender<AudioCommand>) -> Result<Vec<f32>, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx
        .send(AudioCommand::StopRecording { reply_tx })
        .map_err(|_| "Failed to send StopRecording command".to_string())?;

    reply_rx
        .recv()
        .map_err(|_| "Audio thread dropped StopRecording response".to_string())?
}

fn persist_fallback_microphone_selection(requested_device: Option<String>, actual_device: String) {
    tauri::async_runtime::spawn(async move {
        let mut previous_selected = None;
        let update_result = crate::settings::update_settings_atomic(|current_settings| {
            if current_settings.selected_microphone_id.as_ref() != requested_device.as_ref() {
                tracing::debug!(
                    "Skipping microphone auto-update; selection changed from {:?} to {:?}",
                    requested_device,
                    current_settings.selected_microphone_id
                );
                return;
            }

            if current_settings.selected_microphone_id.as_ref() == Some(&actual_device) {
                return;
            }

            previous_selected = current_settings.selected_microphone_id.clone();
            current_settings.selected_microphone_id = Some(actual_device.clone());
        })
        .await;

        match update_result {
            Ok(updated_settings) => {
                if updated_settings.selected_microphone_id.as_ref() != Some(&actual_device) {
                    return;
                }

                tracing::info!(
                    "Auto-updated microphone selection from {:?} to '{}'",
                    previous_selected,
                    actual_device
                );
            }
            Err(e) => {
                tracing::error!("Failed to persist fallback microphone selection: {}", e);
            }
        }
    });
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

async fn init_transcriber_for_variant_async(variant: ModelVariant) -> Result<(), String> {
    let model_path_str = tauri::async_runtime::spawn_blocking(move || {
        ensure_variant_verified(variant).map(|path| path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("Model verification task failed: {e}"))??;

    init_transcriber_async(model_path_str).await
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
        match init_transcriber_for_variant_async(variant).await {
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
    set_recording_start_status(RecordingStartStatus::Idle);
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

    let cmd_tx = match AUDIO_THREAD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Audio thread mutex poisoned on key down, recovering");
            poisoned.into_inner()
        }
    }
    .as_ref()
    .map(|thread| thread.cmd_tx.clone());

    let start_result = if let Some(cmd_tx) = cmd_tx {
        send_start_recording_command(&cmd_tx, settings_snapshot.selected_microphone_id)
    } else {
        Err("Audio thread unavailable".to_string())
    };

    match start_result {
        Ok(()) => {
            set_recording_start_status(RecordingStartStatus::Started { session_id });
        }
        Err(message) => {
            tracing::error!(
                "StartRecording failed for session {}: {}",
                session_id,
                message
            );
            set_recording_start_status(RecordingStartStatus::Failed {
                session_id,
                message,
            });
            return;
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

    let session_id = RECORDING_SESSION_ID.load(Ordering::SeqCst);
    let start_failure = match current_recording_start_status() {
        RecordingStartStatus::Started {
            session_id: started_session_id,
        } if started_session_id == session_id => None,
        RecordingStartStatus::Failed {
            session_id: failed_session_id,
            message,
        } if failed_session_id == session_id => Some(message),
        _ => Some("Recording never started for this session".to_string()),
    };

    if let Some(message) = start_failure {
        set_recording_start_status(RecordingStartStatus::Idle);

        let app_handle = app.clone();
        let test_mode = is_test_mode;
        let duration_ms = duration_ms as i64;
        tauri::async_runtime::spawn(async move {
            crate::notifications::show_error(&app_handle, "Microphone Error", &message);
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
        });
        return;
    }

    set_recording_start_status(RecordingStartStatus::Idle);

    // Transition to Processing (skip state events in test mode)
    if !is_test_mode {
        crate::state::transition_to(crate::state::AppState::Processing).ok();
        app.emit("app-state-changed", "processing").ok();
    }

    // Show processing indicator
    crate::indicator::show_processing(app).ok();

    let duration_ms = duration_ms as i64;

    let cmd_tx = match AUDIO_THREAD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Audio thread mutex poisoned on stop, recovering");
            poisoned.into_inner()
        }
    }
    .as_ref()
    .map(|thread| thread.cmd_tx.clone());

    // Spawn async task for transcription
    let app_handle = app.clone();
    let test_mode = is_test_mode;
    tauri::async_runtime::spawn(async move {
        let stop_result = if let Some(cmd_tx) = cmd_tx {
            match tokio::task::spawn_blocking(move || send_stop_recording_command(&cmd_tx)).await {
                Ok(result) => result,
                Err(e) => Err(format!("StopRecording task failed: {e}")),
            }
        } else {
            Err("Audio thread unavailable".to_string())
        };

        // Get the audio buffer
        let audio_buffer = match stop_result {
            Ok(buf) => buf,
            Err(e) => {
                tracing::error!("No audio buffer received: {}", e);
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

            if let Err(e) = init_transcriber_for_variant_async(settings.active_model_variant).await
            {
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
        tokio::time::sleep(Duration::from_millis(crate::indicator::HIDE_ANIMATION_MS)).await;
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
    if let Err(e) = crate::hotkey_config::set_hotkey_from_string_or_default(&settings.hotkey) {
        tracing::warn!(
            "Failed to parse hotkey from settings: {} - falling back to {}",
            e,
            crate::hotkey_config::DEFAULT_HOTKEY
        );
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn failed_start_does_not_leak_into_next_stop_response() {
        let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCommand>();

        let worker = thread::spawn(move || {
            match cmd_rx.recv().expect("first command should arrive") {
                AudioCommand::StartRecording { reply_tx, .. } => {
                    let _ = reply_tx.send(Err("mic init failed".to_string()));
                }
                AudioCommand::StopRecording { .. } => {
                    panic!("first command should be StartRecording");
                }
            }

            match cmd_rx.recv().expect("second command should arrive") {
                AudioCommand::StartRecording { reply_tx, .. } => {
                    let _ = reply_tx.send(Ok(()));
                }
                AudioCommand::StopRecording { .. } => {
                    panic!("second command should be StartRecording");
                }
            }

            match cmd_rx.recv().expect("third command should arrive") {
                AudioCommand::StopRecording { reply_tx } => {
                    let _ = reply_tx.send(Ok(vec![0.25, 0.5]));
                }
                AudioCommand::StartRecording { .. } => {
                    panic!("third command should be StopRecording");
                }
            }
        });

        assert_eq!(
            send_start_recording_command(&cmd_tx, Some("Broken Mic".to_string())),
            Err("mic init failed".to_string())
        );
        assert_eq!(
            send_start_recording_command(&cmd_tx, Some("Working Mic".to_string())),
            Ok(())
        );
        assert_eq!(send_stop_recording_command(&cmd_tx), Ok(vec![0.25, 0.5]));

        worker.join().expect("worker thread should complete");
    }
}
