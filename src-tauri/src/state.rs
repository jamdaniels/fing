use once_cell::sync::Lazy;
use std::sync::RwLock;

/// Application state machine for the recording lifecycle.
///
/// States: `NeedsSetup` → `Ready` ⇄ `Recording` → `Processing` → `Ready`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    /// First run: model not downloaded, onboarding incomplete.
    NeedsSetup,
    /// Idle and ready to record (hotkey active).
    Ready,
    /// Actively capturing audio while hotkey is held.
    Recording,
    /// Transcribing captured audio.
    Processing,
}

impl AppState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppState::NeedsSetup => "needs-setup",
            AppState::Ready => "ready",
            AppState::Recording => "recording",
            AppState::Processing => "processing",
        }
    }

    pub fn can_record(&self) -> bool {
        matches!(self, AppState::Ready)
    }
}

/// Global application state (thread-safe).
pub static APP_STATE: Lazy<RwLock<AppState>> = Lazy::new(|| RwLock::new(AppState::NeedsSetup));

/// Transition to a new state without emitting frontend events.
pub fn transition_to(new_state: AppState) -> Result<(), &'static str> {
    let mut state = match APP_STATE.write() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("App state write lock poisoned in transition_to, recovering");
            poisoned.into_inner()
        }
    };
    *state = new_state;
    Ok(())
}

/// Set state and emit event to frontend
pub fn set_state(app: &tauri::AppHandle, new_state: AppState) -> Result<(), String> {
    use tauri::Emitter;

    let mut state = match APP_STATE.write() {
        Ok(state) => state,
        Err(poisoned) => {
            tracing::warn!("App state write lock poisoned in set_state, recovering");
            poisoned.into_inner()
        }
    };
    let old_state = *state;
    *state = new_state;
    drop(state);

    tracing::info!("State: {:?} -> {:?}", old_state, new_state);

    app.emit("app-state-changed", new_state.as_str())
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Get current state
pub fn get_state() -> AppState {
    match APP_STATE.read() {
        Ok(state) => *state,
        Err(poisoned) => {
            tracing::warn!("App state read lock poisoned in get_state, recovering");
            *poisoned.into_inner()
        }
    }
}
