// Serialized loading, unloading and hot-swapping of the transcription model.
//
// Every load and unload that can overlap with another one goes through the
// single LOADER lock: the startup preload, the lazy preload on hotkey press,
// the first-use load on hotkey release, the lazy-loading toggle, the idle
// unload and `set_active_model`. Without it an idle unload could drop a model
// that was just swapped in, or a preload that started before a switch could
// load the previous model after it.

use crate::model::{self, ModelVariant};
use crate::settings;
use crate::state::{self, AppState};
use crate::transcribe::{init_transcriber, loaded_variant, unload_transcriber};
use tokio::sync::Mutex;

/// Error returned by `set_active_model` while a recording or transcription
/// is in progress. The frontend maps it to a localized message.
pub const MODEL_SWITCH_BUSY: &str = "model_switch_busy";
/// Error returned by `set_active_model` when the new model failed to load.
pub const MODEL_SWITCH_LOAD_FAILED: &str = "model_switch_load_failed";

static LOADER: Mutex<()> = Mutex::const_new(());

async fn active_variant() -> ModelVariant {
    settings::load_settings().await.active_model_variant
}

/// Load `variant`, replacing any other engine. Caller holds the LOADER lock.
async fn load_locked(variant: ModelVariant) -> Result<(), String> {
    if loaded_variant() == Some(variant) {
        return Ok(());
    }

    let model_path =
        tauri::async_runtime::spawn_blocking(move || model::ensure_variant_available(variant))
            .await
            .map_err(|e| format!("Model inspection task failed: {e}"))??;

    // `init_transcriber` logs the load time.
    tauri::async_runtime::spawn_blocking(move || init_transcriber(variant, &model_path))
        .await
        .map_err(|e| format!("Transcriber initialization task failed: {e}"))?
        .map_err(|e| e.to_string())
}

/// Make sure the active model is loaded. Returns at once when it already is;
/// otherwise the active model is read under the lock, so a load requested
/// before a model switch never reloads the previous model.
pub async fn ensure_loaded() -> Result<(), String> {
    if loaded_variant() == Some(active_variant().await) {
        return Ok(());
    }
    let _guard = LOADER.lock().await;
    load_locked(active_variant().await).await
}

/// After `variant` was downloaded: if it is the active model (its file had
/// gone missing) and the app is ready, load it now rather than on the next
/// hotkey press. Onboarding loads its model on setup completion instead.
pub async fn load_if_active(variant: ModelVariant) {
    let settings = settings::load_settings().await;
    if state::get_state() != AppState::Ready
        || settings.active_model_variant != variant
        || settings.lazy_model_loading
    {
        return;
    }
    if let Err(e) = ensure_loaded().await {
        tracing::warn!("Loading the downloaded active model failed: {}", e);
    }
}

/// Unload the model (lazy loading was turned on).
pub async fn unload() {
    let _guard = LOADER.lock().await;
    unload_transcriber();
}

/// Idle unload for lazy loading. `still_idle` is checked once the lock is
/// held, so a load that was in flight (lazy preload, lazy toggle off, model
/// switch) finishes first and the decision is made on the current state.
/// Returns whether the model was unloaded.
pub async fn unload_if_idle(still_idle: impl FnOnce() -> bool) -> bool {
    let _guard = LOADER.lock().await;
    if !still_idle() || !settings::load_settings().await.lazy_model_loading {
        return false;
    }
    unload_transcriber();
    true
}

/// Full SHA-256 check of `variant` (cached per file), kept off the runtime.
/// The error can name the models folder, so callers return it, not log it.
pub async fn verify(variant: ModelVariant) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || model::ensure_variant_verified(variant))
        .await
        .map_err(|e| format!("Model verification task failed: {e}"))??;
    Ok(())
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

    verify(variant).await?;

    let _guard = LOADER.lock().await;
    // A recording may have started while this waited for the lock.
    ensure_not_busy()?;

    let mut previous = variant;
    let saved = settings::update_settings_atomic(|latest| {
        previous = latest.active_model_variant;
        latest.active_model_variant = variant;
    })
    .await?;

    let loaded = loaded_variant();
    let engine_loaded = loaded.is_some();
    if loaded == Some(variant) {
        tracing::info!("Model {:?} is already active and loaded", variant);
        return Ok(());
    }
    if engine_loaded {
        unload_transcriber();
        tracing::info!("Unloaded {:?} model for switch to {:?}", previous, variant);
    }

    // Before onboarding (NeedsSetup) the model loads on setup completion;
    // with lazy loading it loads on the next hotkey press.
    if saved.lazy_model_loading || state::get_state() == AppState::NeedsSetup {
        tracing::info!("Active model set to {:?}; load deferred", variant);
        return Ok(());
    }

    match load_locked(variant).await {
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
                    if let Err(restore_err) = load_locked(previous).await {
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
