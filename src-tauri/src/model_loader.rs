// Serialized loading, unloading and hot-swapping of the transcription model.
//
// Every load and unload that can overlap with another one goes through the
// single LOADER lock: the startup preload, the lazy preload on hotkey press,
// the first-use load on hotkey release, the lazy-loading toggle, the idle
// unload and `set_active_model`. Without it an idle unload could drop a model
// that was just swapped in, or a preload that started before a switch could
// leave the previous model loaded (`init_transcriber` keeps whatever engine is
// already loaded).

use crate::model::{self, ModelVariant};
use crate::settings;
use crate::state::{self, AppState};
use crate::transcribe::{init_transcriber, is_transcriber_loaded, unload_transcriber};
use std::time::Instant;
use tokio::sync::Mutex;

/// Error returned by `set_active_model` while a recording or transcription
/// is in progress. The frontend maps it to a localized message.
pub const MODEL_SWITCH_BUSY: &str = "model_switch_busy";
/// Error returned by `set_active_model` when the new model failed to load.
pub const MODEL_SWITCH_LOAD_FAILED: &str = "model_switch_load_failed";

/// The variant of the engine loaded through this module. It is only
/// trusted while `is_transcriber_loaded()` agrees.
static LOADER: Mutex<Option<ModelVariant>> = Mutex::const_new(None);

async fn active_variant() -> ModelVariant {
    settings::load_settings().await.active_model_variant
}

/// Load `variant`, replacing any other engine. Caller holds the LOADER lock.
async fn load_locked(
    loaded: &mut Option<ModelVariant>,
    variant: ModelVariant,
) -> Result<(), String> {
    if is_transcriber_loaded() {
        if *loaded == Some(variant) {
            return Ok(());
        }
        // A different (or untracked) engine is loaded: drop it first, since
        // init_transcriber would otherwise keep it.
        unload_transcriber();
    }
    *loaded = None;

    let model_path = tauri::async_runtime::spawn_blocking(move || {
        model::ensure_variant_available(variant).map(|path| path.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("Model inspection task failed: {e}"))??;

    let started = Instant::now();
    tauri::async_runtime::spawn_blocking(move || init_transcriber(&model_path))
        .await
        .map_err(|e| format!("Transcriber initialization task failed: {e}"))?
        .map_err(|e| e.to_string())?;

    *loaded = Some(variant);
    tracing::info!(
        "Loaded {:?} model in {} ms",
        variant,
        started.elapsed().as_millis()
    );
    Ok(())
}

fn unload_locked(loaded: &mut Option<ModelVariant>) {
    unload_transcriber();
    *loaded = None;
}

/// Make sure the active model is loaded. `requested` is the caller's
/// settings snapshot; the active model is re-read under the lock so a
/// snapshot taken before a model switch never reloads the previous model.
pub async fn ensure_loaded(requested: ModelVariant) -> Result<(), String> {
    let mut loaded = LOADER.lock().await;
    let variant = active_variant().await;
    if variant != requested {
        tracing::info!(
            "Active model changed to {:?} since {:?} was requested; loading the active model",
            variant,
            requested
        );
    }
    load_locked(&mut loaded, variant).await
}

/// Unload the model (lazy loading was turned on).
pub async fn unload() {
    let mut loaded = LOADER.lock().await;
    unload_locked(&mut loaded);
}

/// Idle unload for lazy loading. `still_idle` is checked once the lock is
/// held, so a load that was in flight (lazy preload, lazy toggle off, model
/// switch) finishes first and the decision is made on the current state.
/// Returns whether the model was unloaded.
pub async fn unload_if_idle(still_idle: impl FnOnce() -> bool) -> bool {
    let mut loaded = LOADER.lock().await;
    if !still_idle() || !settings::load_settings().await.lazy_model_loading {
        return false;
    }
    unload_locked(&mut loaded);
    true
}

fn ensure_not_busy() -> Result<(), String> {
    match state::get_state() {
        AppState::Recording | AppState::Processing => Err(MODEL_SWITCH_BUSY.to_string()),
        AppState::NeedsSetup | AppState::Ready => Ok(()),
    }
}

/// Make `variant` the active model and hot-swap the engine without a
/// restart: the current engine is unloaded, then the new one loads off the
/// async runtime while the app stays Ready. Rejected while recording or
/// transcribing. With lazy loading on, the switch only unloads the current
/// engine and the next hotkey press loads the new model.
pub async fn switch_active_model(variant: ModelVariant) -> Result<(), String> {
    ensure_not_busy()?;

    // Full SHA-256 check (cached per file); hashing stays off the runtime.
    // The error can name the models folder, so it is returned, not logged.
    tauri::async_runtime::spawn_blocking(move || model::ensure_variant_verified(variant))
        .await
        .map_err(|e| format!("Model verification task failed: {e}"))??;

    let mut loaded = LOADER.lock().await;
    // A recording may have started while this waited for the lock.
    ensure_not_busy()?;

    let mut previous = variant;
    let saved = settings::update_settings_atomic(|latest| {
        previous = latest.active_model_variant;
        latest.active_model_variant = variant;
    })
    .await?;

    let engine_loaded = is_transcriber_loaded();
    if engine_loaded && *loaded == Some(variant) {
        tracing::info!("Model {:?} is already active and loaded", variant);
        return Ok(());
    }
    if engine_loaded {
        unload_locked(&mut loaded);
        tracing::info!("Unloaded {:?} model for switch to {:?}", previous, variant);
    }

    // Before onboarding (NeedsSetup) the model loads on setup completion;
    // with lazy loading it loads on the next hotkey press.
    if saved.lazy_model_loading || state::get_state() == AppState::NeedsSetup {
        tracing::info!("Active model set to {:?}; load deferred", variant);
        return Ok(());
    }

    match load_locked(&mut loaded, variant).await {
        Ok(()) => {
            tracing::info!("Switched active model from {:?} to {:?}", previous, variant);
            Ok(())
        }
        Err(load_err) => {
            tracing::error!("Failed to load {:?} model: {}", variant, load_err);
            if previous != variant {
                if let Err(save_err) = settings::update_settings_atomic(|latest| {
                    latest.active_model_variant = previous;
                })
                .await
                {
                    tracing::error!("Failed to restore previous active model: {}", save_err);
                } else if engine_loaded {
                    // Best effort: restore the engine that was loaded before.
                    if let Err(restore_err) = load_locked(&mut loaded, previous).await {
                        tracing::warn!(
                            "Failed to reload {:?} model after a failed switch: {}",
                            previous,
                            restore_err
                        );
                    }
                }
            }
            Err(MODEL_SWITCH_LOAD_FAILED.to_string())
        }
    }
}
