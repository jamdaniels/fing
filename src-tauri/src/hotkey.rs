use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::audio::{is_digital_silence, AudioCapture, AudioTap};
use crate::db::{save_transcript, NewTranscript};
use crate::engine::{AbortFlag, TranscribeError};
use crate::indicator::NoticeKind;
use crate::microphone::{self, MicChoice, OpenedMic};
use crate::paste::paste_text;
use crate::settings::{load_settings, load_settings_sync};
use crate::sounds;
use crate::transcribe::{is_blank_audio, is_transcriber_loaded, loaded_variant};

// Maximum recording duration; the audio buffer is sized for the same limit.
const MAX_RECORDING_DURATION: Duration =
    Duration::from_secs(crate::audio::MAX_RECORDING_DURATION_SECS as u64);
const LAZY_MODEL_IDLE_UNLOAD_SECS: u64 = 10;

// Whether a recording is in progress (hotkey held or hands-free)
static RECORDING_ACTIVE: AtomicBool = AtomicBool::new(false);

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

// Commands sent to the audio thread
enum AudioCommand {
    StartRecording {
        choice: MicChoice,
        reply_tx: Sender<Result<(AudioTap, OpenedMic), String>>,
    },
    StopRecording {
        reply_tx: Sender<Result<Vec<f32>, String>>,
    },
    /// Stop capturing and drop the audio without resampling it.
    Discard,
}

/// The current recording as seen by the hotkey gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordingSession {
    Inactive,
    Live,
    /// Recording, but the microphone failed to open.
    Failed,
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
static RECORDING_LIFECYCLE: Mutex<()> = Mutex::new(());

// Recording start time for duration tracking
static RECORDING_START: Mutex<Option<Instant>> = Mutex::new(None);

// Frontmost app when recording started (to restore focus before paste)
#[cfg(target_os = "macos")]
static FRONTMOST_APP: Mutex<Option<String>> = Mutex::new(None);

/// Abort flag of the current recording, from key down until its text is
/// about to be pasted. Escape sets it while recording or transcribing;
/// whisper.cpp then stops within one encoder run or decoder step.
static SESSION_ABORT: Mutex<Option<AbortFlag>> = Mutex::new(None);

fn lock_session_abort() -> MutexGuard<'static, Option<AbortFlag>> {
    match SESSION_ABORT.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Session abort mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Abort the current recording's transcriptions, if it still can be.
/// Returns whether there was one to abort.
fn abort_session() -> bool {
    let mut slot = lock_session_abort();
    // Set under the lock, so `claim_for_paste` can't miss it.
    match slot.take() {
        Some(abort) => {
            abort.abort();
            true
        }
        None => false,
    }
}

/// Close the cancel window of the recording behind `abort` just before its
/// text is pasted. False when Escape aborted it first.
fn claim_for_paste(abort: &AbortFlag) -> bool {
    let mut slot = lock_session_abort();
    if slot.as_ref().is_some_and(|current| current.same(abort)) {
        *slot = None;
    }
    !abort.is_aborted()
}

/// Whether Escape can still cancel the transcription in progress (its text
/// is not being pasted yet).
pub(crate) fn is_transcription_cancellable() -> bool {
    crate::state::get_state() == crate::state::AppState::Processing
        && lock_session_abort().is_some()
}

/// Abort any transcription still running (Escape, app exit), including a
/// recording's pre-transcription chunk.
pub fn abort_transcriptions() {
    abort_session();
    crate::pretranscribe::cancel();
}

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
                Ok(AudioCommand::StartRecording { choice, reply_tx }) => {
                    let opened = match microphone::open(&mut capture, &choice) {
                        Ok(opened) => opened,
                        Err(e) => {
                            tracing::error!("Failed to init audio capture: {}", e);
                            let _ = reply_tx.send(Err(format!("Unable to start recording: {e}")));
                            continue;
                        }
                    };

                    capture.begin_recording();
                    tracing::info!("Audio recording started");
                    if opened.fell_back {
                        tracing::warn!(
                            "Selected microphone unavailable, recording with '{}'",
                            opened.name
                        );
                    }
                    let _ = reply_tx.send(Ok((capture.tap(), opened)));
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
                Ok(AudioCommand::Discard) => {
                    drop(capture.end_recording());
                    capture.close_capture();
                    tracing::info!("Audio recording cancelled, buffer discarded");
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

/// Whether a recording is in progress, from its start until it stops or is
/// cancelled.
pub(crate) fn is_recording_active() -> bool {
    RECORDING_ACTIVE.load(Ordering::SeqCst)
}

pub(crate) fn recording_session() -> RecordingSession {
    let _lifecycle_guard = lock_recording_lifecycle();
    if !RECORDING_ACTIVE.load(Ordering::SeqCst) {
        return RecordingSession::Inactive;
    }
    let session_id = RECORDING_SESSION_ID.load(Ordering::SeqCst);
    match current_recording_start_status() {
        RecordingStartStatus::Failed {
            session_id: failed_session_id,
            ..
        } if failed_session_id == session_id => RecordingSession::Failed,
        _ => RecordingSession::Live,
    }
}

fn lock_recording_lifecycle() -> MutexGuard<'static, ()> {
    match RECORDING_LIFECYCLE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Recording lifecycle mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// The audio thread's command sender, if the thread is running.
fn audio_cmd_tx(context: &str) -> Option<Sender<AudioCommand>> {
    match AUDIO_THREAD.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("Audio thread mutex poisoned on {}, recovering", context);
            poisoned.into_inner()
        }
    }
    .as_ref()
    .map(|thread| thread.cmd_tx.clone())
}

fn send_start_recording_command(
    cmd_tx: &Sender<AudioCommand>,
    choice: MicChoice,
) -> Result<(AudioTap, OpenedMic), String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    cmd_tx
        .send(AudioCommand::StartRecording { choice, reply_tx })
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

fn mark_lazy_activity() -> u64 {
    LAZY_ACTIVITY_TOKEN.fetch_add(1, Ordering::SeqCst) + 1
}

fn spawn_lazy_preload_if_needed() {
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
        // Loads go through model_loader so they serialize with model
        // switches, the startup preload and the idle unload.
        match crate::model_loader::ensure_loaded().await {
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

        let still_idle = move || {
            LAZY_ACTIVITY_TOKEN.load(Ordering::SeqCst) == token
                && !RECORDING_ACTIVE.load(Ordering::SeqCst)
                && crate::state::get_state() == crate::state::AppState::Ready
        };
        if !still_idle() {
            return;
        }

        // Re-checked under the model lock (with the lazy-loading setting), so
        // a load in flight (e.g. a model switch) completes first and is not
        // unloaded mid-load.
        if !crate::model_loader::unload_if_idle(still_idle).await {
            return;
        }
        tracing::info!(
            "Unloaded transcriber after {} seconds of idle time",
            LAZY_MODEL_IDLE_UNLOAD_SECS
        );
    });
}

/// Start recording (hotkey pressed)
pub fn on_key_down(app: &AppHandle) {
    let _lifecycle_guard = lock_recording_lifecycle();
    let is_test_mode = ONBOARDING_TEST_MODE.load(Ordering::SeqCst);

    if RECORDING_ACTIVE.load(Ordering::SeqCst) {
        return;
    }

    let settings_snapshot = load_settings_sync();

    // Onboarding and model repair make sure the active model is on disk, but
    // the file can still go missing while the app runs. Don't record audio
    // that can't be transcribed; Settings > Models offers the download. Only
    // an existence check here: the load inspects the file fully.
    let active = settings_snapshot.active_model_variant;
    if !is_test_mode
        && loaded_variant() != Some(active)
        && !crate::model::model_path_for_variant(active).exists()
    {
        tracing::warn!("Active {:?} model is not downloaded; not recording", active);
        notify(
            app,
            NoticeKind::Error,
            &crate::i18n::current().indicator.model_missing,
        );
        return;
    }

    if !is_test_mode && settings_snapshot.lazy_model_loading {
        // Invalidate any pending lazy unload timer from older activity.
        mark_lazy_activity();
        spawn_lazy_preload_if_needed();
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
        if !crate::state::try_transition(
            crate::state::AppState::Ready,
            crate::state::AppState::Recording,
        ) {
            return;
        }
        app.emit("app-state-changed", "recording").ok();
    }
    RECORDING_ACTIVE.store(true, Ordering::SeqCst);

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

    // Increment session ID and start auto-stop timer
    let session_id = RECORDING_SESSION_ID.fetch_add(1, Ordering::SeqCst) + 1;
    set_recording_start_status(RecordingStartStatus::Idle);
    let abort = AbortFlag::default();
    *lock_session_abort() = Some(abort.clone());
    let app_for_timer = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(MAX_RECORDING_DURATION).await;

        // Only trigger auto-stop if this session is still active
        let current_session = RECORDING_SESSION_ID.load(Ordering::SeqCst);
        if current_session == session_id && RECORDING_ACTIVE.load(Ordering::SeqCst) {
            tracing::info!(
                "Auto-stopping recording after {} seconds",
                MAX_RECORDING_DURATION.as_secs()
            );
            notify(
                &app_for_timer,
                NoticeKind::Info,
                &crate::i18n::current().indicator.recording_limit_reached,
            );
            on_key_up(&app_for_timer);
        }
    });

    // Open the sound output while the mic starts; the cue plays once it is live.
    let start_cue = settings_snapshot.sound_enabled.then(sounds::prepare_start);

    // Ensure audio thread is running and start recording
    ensure_audio_thread();

    let cmd_tx = audio_cmd_tx("key down");

    let mic_choice = MicChoice::from_settings(&settings_snapshot);
    let start_result = if let Some(cmd_tx) = cmd_tx {
        send_start_recording_command(&cmd_tx, mic_choice.clone())
    } else {
        Err("Audio thread unavailable".to_string())
    };

    match start_result {
        Ok((tap, opened)) => {
            set_recording_start_status(RecordingStartStatus::Started { session_id });
            crate::pretranscribe::start(
                tap.clone(),
                session_id,
                settings_snapshot.languages.clone(),
                crate::dictionary::build_prompt(&settings_snapshot.dictionary_terms),
                abort,
            );
            crate::level_meter::start(app, tap);
            if let Some(cue) = start_cue {
                cue.play();
            }
            after_microphone_opened(app, mic_choice, opened);
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
        }
    }
}

/// Announces a mic change in the pill (recording keeps going) and saves any
/// automatic microphone settings change, off the hotkey path.
fn after_microphone_opened(app: &AppHandle, choice: MicChoice, opened: OpenedMic) {
    if microphone::note_recording_mic(&opened) {
        let message = crate::i18n::current()
            .indicator
            .using_microphone
            .replace("{name}", &opened.name);
        notify(app, NoticeKind::Info, &message);
    }
    if let Some(next) = opened.settings_update {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            microphone::persist_update(&app, choice, next).await;
        });
    }
}

/// Stop recording and transcribe (hotkey released, or tapped in hands-free)
pub fn on_key_up(app: &AppHandle) {
    let _lifecycle_guard = lock_recording_lifecycle();
    let is_test_mode = ONBOARDING_TEST_MODE.load(Ordering::SeqCst);

    if !RECORDING_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let released = Instant::now();

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
            tracing::warn!("Recording did not start: {}", message);
            notify(
                &app_handle,
                NoticeKind::Error,
                &crate::i18n::current().indicator.microphone_unavailable,
            );
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
        });
        return;
    }

    set_recording_start_status(RecordingStartStatus::Idle);

    // Transition to Processing (skip state events in test mode). If another
    // subsystem changed the state unexpectedly, still stop the captured audio
    // below, but do not emit a misleading Processing event.
    let processing_claimed = is_test_mode
        || crate::state::try_transition(
            crate::state::AppState::Recording,
            crate::state::AppState::Processing,
        );
    if !is_test_mode && processing_claimed {
        app.emit("app-state-changed", "processing").ok();
    } else if !processing_claimed {
        tracing::warn!("Recording stop could not claim the Processing state");
    }

    if processing_claimed {
        crate::indicator::show_processing(app).ok();
    }

    let duration_ms = duration_ms as i64;

    // Stop pre-transcription first, so it starts no new chunk.
    let pretranscription = crate::pretranscribe::stop(session_id);
    // Escape sets it while this transcribes (see `on_transcription_cancel`).
    let abort = lock_session_abort().clone().unwrap_or_default();
    let cmd_tx = audio_cmd_tx("stop");

    // Spawn async task for transcription
    let app_handle = app.clone();
    let test_mode = is_test_mode;
    tauri::async_runtime::spawn(async move {
        // Stop the recording while a chunk still in flight finishes (the
        // engine runs one call at a time): spawned now, awaited after the wait.
        let stop = cmd_tx.map(|cmd_tx| {
            tokio::task::spawn_blocking(move || send_stop_recording_command(&cmd_tx))
        });
        let pretranscribed = match pretranscription {
            Some(stopped) => tokio::task::spawn_blocking(move || stopped.wait())
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let stop_result = match stop {
            Some(task) => match task.await {
                Ok(result) => result,
                Err(e) => Err(format!("StopRecording task failed: {e}")),
            },
            None => Err("Audio thread unavailable".to_string()),
        };

        if abort.is_aborted() {
            finish_cancelled(&app_handle, duration_ms, test_mode).await;
            return;
        }

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

        if is_digital_silence(&audio_buffer) {
            tracing::info!("Recording is digital silence, skipping transcription");
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
            return;
        }

        tracing::info!(
            "Processing {} samples ({:.1}s of audio)",
            audio_buffer.len(),
            audio_buffer.len() as f32 / crate::audio::WHISPER_SAMPLE_RATE as f32
        );

        // Load settings for model variant and language
        let settings = load_settings().await;

        // Make sure the active model is the one loaded. This returns at once
        // when it already is; otherwise it loads it, replacing any other
        // engine (e.g. a model picked again during onboarding after a test).
        if let Err(e) = crate::model_loader::ensure_loaded().await {
            tracing::error!("Failed to initialize transcriber: {}", e);
            notify(
                &app_handle,
                NoticeKind::Error,
                &crate::i18n::current().indicator.model_load_failed,
            );
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
            return;
        }

        let dictionary_terms = crate::dictionary::sanitize_terms(&settings.dictionary_terms);
        let dictionary_prompt = crate::dictionary::build_prompt(&dictionary_terms);

        // Transcribe (only the tail when chunks were pre-transcribed)
        let text = match crate::pretranscribe::transcribe_recording(
            pretranscribed,
            &audio_buffer,
            &settings.languages,
            dictionary_prompt.as_deref(),
            released,
            &abort,
        ) {
            Ok(t) => t,
            Err(TranscribeError::Aborted) => {
                finish_cancelled(&app_handle, duration_ms, test_mode).await;
                return;
            }
            Err(e) => {
                tracing::error!("Transcription failed: {}", e);
                notify(
                    &app_handle,
                    NoticeKind::Error,
                    &crate::i18n::current().indicator.transcription_failed,
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
        if is_blank_audio(&text) {
            tracing::info!("No speech detected, skipping paste/save");
            finish_transcription(&app_handle, None, duration_ms, test_mode).await;
            return;
        }

        // Past this point Escape no longer cancels.
        if !claim_for_paste(&abort) {
            finish_cancelled(&app_handle, duration_ms, test_mode).await;
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

/// Called when Escape is pressed while recording: stop recording and discard
/// the audio, so nothing is transcribed, pasted or saved.
pub fn on_key_cancel(app: &AppHandle) {
    let _lifecycle_guard = lock_recording_lifecycle();
    let is_test_mode = ONBOARDING_TEST_MODE.load(Ordering::SeqCst);

    if !RECORDING_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }

    tracing::info!("Recording cancelled by Escape");
    abort_transcriptions();

    let session_id = RECORDING_SESSION_ID.load(Ordering::SeqCst);
    let recording_started = matches!(
        current_recording_start_status(),
        RecordingStartStatus::Started { session_id: started_session_id }
            if started_session_id == session_id
    );
    set_recording_start_status(RecordingStartStatus::Idle);

    if recording_started {
        // The audio thread handles commands in order, so the next
        // StartRecording can't overtake this.
        let sent =
            audio_cmd_tx("cancel").is_some_and(|cmd_tx| cmd_tx.send(AudioCommand::Discard).is_ok());
        if !sent {
            tracing::warn!("Failed to send Discard command");
        }
    }

    clear_frontmost_app();

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        finish_transcription(&app_handle, None, 0, is_test_mode).await;
    });
}

/// Called when Escape is pressed while a recording is being transcribed:
/// abort the transcription so nothing is pasted or saved. The task started
/// by `on_key_up` sees the abort and returns to Ready.
pub fn on_transcription_cancel(app: &AppHandle) {
    if crate::state::get_state() != crate::state::AppState::Processing || !abort_session() {
        return;
    }
    tracing::info!("Transcription cancelled by Escape");
    // Hide now: the task may still be loading the model or finishing an
    // encoder run.
    crate::indicator::hide(app).ok();
}

/// End a transcription that Escape aborted: nothing is pasted or saved.
async fn finish_cancelled(app: &AppHandle, duration_ms: i64, is_test_mode: bool) {
    tracing::info!("Transcription aborted; nothing pasted");
    clear_frontmost_app();
    finish_transcription(app, None, duration_ms, is_test_mode).await;
}

fn clear_frontmost_app() {
    #[cfg(target_os = "macos")]
    {
        match FRONTMOST_APP.lock() {
            Ok(app) => app,
            Err(poisoned) => poisoned.into_inner(),
        }
        .take();
    }
}

/// Show a notice in the indicator; error details stay in the logs.
fn notify(app: &AppHandle, kind: NoticeKind, message: &str) {
    if let Err(e) = crate::indicator::notify(app, kind, message) {
        tracing::warn!("Failed to show indicator notice: {}", e);
    }
}

async fn finish_transcription(
    app: &AppHandle,
    text: Option<String>,
    _duration_ms: i64,
    is_test_mode: bool,
) {
    // The recording is over; Escape has nothing left to cancel.
    lock_session_abort().take();

    // Hide indicator
    crate::indicator::hide(app).ok();

    // Return to Ready (skip state events in test mode)
    if !is_test_mode {
        tokio::time::sleep(Duration::from_millis(crate::indicator::HIDE_ANIMATION_MS)).await;
        crate::state::transition_to(crate::state::AppState::Ready).ok();
        app.emit("app-state-changed", "ready").ok();
        // Defensive: drop stale listener state so the next press always works,
        // while keeping keys the user is still physically holding.
        crate::hotkey_listener::resync_listener_state();
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
    if let Err(e) = crate::hotkey_config::set_hotkey_from_string(&settings.hotkey) {
        tracing::warn!(
            "Failed to parse hotkey from settings: {} - onboarding hotkey test will remain inactive",
            e
        );
        crate::hotkey_config::clear_hotkey_config()?;
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
                AudioCommand::StopRecording { .. } | AudioCommand::Discard => {
                    panic!("first command should be StartRecording");
                }
            }

            match cmd_rx.recv().expect("second command should arrive") {
                AudioCommand::StartRecording { reply_tx, .. } => {
                    let opened = OpenedMic {
                        id: "working".to_string(),
                        name: "Working Mic".to_string(),
                        fell_back: false,
                        settings_update: None,
                    };
                    let _ = reply_tx.send(Ok((AudioCapture::new().tap(), opened)));
                }
                AudioCommand::StopRecording { .. } | AudioCommand::Discard => {
                    panic!("second command should be StartRecording");
                }
            }

            match cmd_rx.recv().expect("third command should arrive") {
                AudioCommand::StopRecording { reply_tx } => {
                    let _ = reply_tx.send(Ok(vec![0.25, 0.5]));
                }
                AudioCommand::StartRecording { .. } | AudioCommand::Discard => {
                    panic!("third command should be StopRecording");
                }
            }
        });

        assert_eq!(
            send_start_recording_command(&cmd_tx, MicChoice::default()).err(),
            Some("mic init failed".to_string())
        );
        assert!(send_start_recording_command(&cmd_tx, MicChoice::default()).is_ok());
        assert_eq!(send_stop_recording_command(&cmd_tx), Ok(vec![0.25, 0.5]));

        worker.join().expect("worker thread should complete");
    }
}
