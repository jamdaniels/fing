use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::audio::AudioCapture;
use crate::db::{save_transcript, NewTranscript};
use crate::model::default_model_path;
use crate::paste::set_clipboard_and_paste;
use crate::settings::{load_settings, load_settings_sync};
use crate::sounds;
use crate::transcribe::{get_transcriber, init_transcriber, transcribe_audio};

// Minimum recording duration in milliseconds
const MIN_RECORDING_DURATION_MS: u64 = 200;
// Maximum recording duration (2 minutes) in milliseconds
const MAX_RECORDING_DURATION_MS: u64 = 2 * 60 * 1000;

// Track if key is currently held
static KEY_HELD: AtomicBool = AtomicBool::new(false);

// Recording session ID to track auto-stop timer validity
static RECORDING_SESSION_ID: AtomicU64 = AtomicU64::new(0);

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

/// Initialize the audio thread (call once at startup or before first recording)
fn ensure_audio_thread() {
    let mut thread_guard = AUDIO_THREAD.lock().unwrap();
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
                        let _ = resp_tx.send(AudioResponse {
                            buffer: Vec::new(),
                        });
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

/// Called when F8 is pressed down
pub fn on_key_down(app: &AppHandle) {
    // Check current state - only proceed if Ready
    let state = crate::state::APP_STATE.read().unwrap();
    if !state.can_record() {
        return;
    }
    drop(state);

    // Transition to Recording
    crate::state::transition_to(crate::state::AppState::Recording).ok();
    KEY_HELD.store(true, Ordering::SeqCst);

    // Emit state change event
    app.emit("app-state-changed", "recording").ok();

    // Show recording indicator
    crate::indicator::show_recording(app).ok();

    // Store recording start time
    {
        let mut start = RECORDING_START.lock().unwrap();
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

    if let Some(ref thread) = *AUDIO_THREAD.lock().unwrap() {
        let selected_device_id = load_settings_sync().selected_microphone_id;
        if thread
            .cmd_tx
            .send(AudioCommand::StartRecording(selected_device_id))
            .is_err()
        {
            tracing::error!("Failed to send StartRecording command");
        }
    }

    // Play start sound if enabled (load settings synchronously via spawn_blocking)
    let app_clone = app.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let settings = rt.block_on(load_settings());
        if settings.sound_enabled {
            sounds::play_start();
        }
        drop(app_clone);
    });
}

/// Called when F8 is released
pub fn on_key_up(app: &AppHandle) {
    if !KEY_HELD.load(Ordering::SeqCst) {
        return;
    }
    KEY_HELD.store(false, Ordering::SeqCst);

    // Check we're in Recording state
    let state = crate::state::APP_STATE.read().unwrap();
    if !matches!(*state, crate::state::AppState::Recording) {
        return;
    }
    drop(state);

    // Calculate recording duration
    let duration_ms = {
        let start = RECORDING_START.lock().unwrap();
        start.map(|s| s.elapsed().as_millis() as u64).unwrap_or(0)
    };

    // Minimum recording duration guard (200ms)
    if duration_ms < MIN_RECORDING_DURATION_MS {
        tracing::info!("Recording too short ({}ms), ignoring", duration_ms);
        crate::notifications::show_info(app, "Fing", "Recording too short");
        // Hide indicator and return to Ready
        crate::indicator::hide(app).ok();
        crate::state::transition_to(crate::state::AppState::Ready).ok();
        app.emit("app-state-changed", "ready").ok();
        // Still need to stop recording to reset audio state and drain response
        {
            let thread_guard = AUDIO_THREAD.lock().unwrap();
            if let Some(ref thread) = *thread_guard {
                let _ = thread.cmd_tx.send(AudioCommand::StopRecording);
            }
        }
        // Drain the response in a background thread to avoid blocking
        std::thread::spawn(|| {
            let thread_guard = AUDIO_THREAD.lock().unwrap();
            if let Some(ref thread) = *thread_guard {
                let _ = thread.resp_rx.recv();
            }
        });
        return;
    }

    // Transition to Processing
    crate::state::transition_to(crate::state::AppState::Processing).ok();
    app.emit("app-state-changed", "processing").ok();

    // Show processing indicator
    crate::indicator::show_processing(app).ok();

    let duration_ms = duration_ms as i64;

    // Stop recording and get audio buffer
    // We need to send command and then receive response
    let cmd_sent = {
        let thread_guard = AUDIO_THREAD.lock().unwrap();
        if let Some(ref thread) = *thread_guard {
            thread.cmd_tx.send(AudioCommand::StopRecording).is_ok()
        } else {
            false
        }
    };

    // Spawn async task for transcription
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Wait for audio response (blocking recv in async context via spawn_blocking)
        let audio_buffer = if cmd_sent {
            tokio::task::spawn_blocking(|| {
                let thread_guard = AUDIO_THREAD.lock().unwrap();
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
                finish_transcription(&app_handle, None, duration_ms).await;
                return;
            }
        };

        if audio_buffer.is_empty() {
            tracing::warn!("Empty audio buffer");
            finish_transcription(&app_handle, None, duration_ms).await;
            return;
        }

        tracing::info!(
            "Processing {} samples ({:.1}s of audio)",
            audio_buffer.len(),
            audio_buffer.len() as f32 / 16000.0
        );

        // Initialize transcriber if needed
        if get_transcriber().is_none() {
            let model_path = default_model_path();
            let model_path_str = model_path.to_string_lossy().to_string();

            // Check if model exists
            if !model_path.exists() {
                tracing::error!("Model not found at {:?}", model_path);
                crate::notifications::show_error(
                    &app_handle,
                    "Model Not Found",
                    "Please download the model in settings",
                );
                finish_transcription(&app_handle, None, duration_ms).await;
                return;
            }

            if let Err(e) = init_transcriber(&model_path_str) {
                tracing::error!("Failed to initialize transcriber: {}", e);
                crate::notifications::show_error(
                    &app_handle,
                    "Model Error",
                    &format!("Failed to load model: {}", e),
                );
                finish_transcription(&app_handle, None, duration_ms).await;
                return;
            }
            tracing::info!("Transcriber initialized from {:?}", model_path);
        }

        // Transcribe
        let text = match transcribe_audio(&audio_buffer) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Transcription failed: {}", e);
                crate::notifications::show_error(
                    &app_handle,
                    "Transcription Error",
                    &format!("{}", e),
                );
                finish_transcription(&app_handle, None, duration_ms).await;
                return;
            }
        };

        // Handle empty or whitespace-only transcription
        let text = text.trim().to_string();
        if text.is_empty() {
            tracing::warn!("Transcription returned empty text");
            finish_transcription(&app_handle, None, duration_ms).await;
            return;
        }

        tracing::info!("Transcription result: {}", text);

        // Load settings
        let settings = load_settings().await;

        // Clipboard and paste
        if settings.paste_enabled {
            let paste_result = set_clipboard_and_paste(&text);
            if paste_result.should_notify() {
                crate::notifications::show_clipboard_fallback(&app_handle);
            }
        }

        // Save to history if enabled
        if settings.history_enabled {
            let transcript = NewTranscript {
                text: text.clone(),
                duration_ms,
                app_context: None, // TODO: Get focused app context
            };
            if let Err(e) = save_transcript(&transcript) {
                tracing::error!("Failed to save transcript: {}", e);
            }
        }

        // Play done sound if enabled
        if settings.sound_enabled {
            sounds::play_done();
        }

        finish_transcription(&app_handle, Some(text), duration_ms).await;
    });
}

async fn finish_transcription(app: &AppHandle, text: Option<String>, _duration_ms: i64) {
    // Hide indicator
    crate::indicator::hide(app).ok();

    // Return to Ready
    crate::state::transition_to(crate::state::AppState::Ready).ok();
    app.emit("app-state-changed", "ready").ok();

    if let Some(t) = text {
        app.emit("transcript-added", t).ok();
    }
}

/// Register the global hotkey (F8)
pub fn register_hotkey(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::register_global_hotkey(app.clone())
    }

    #[cfg(not(target_os = "macos"))]
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
