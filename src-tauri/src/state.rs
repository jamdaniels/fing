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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static STATE_TEST_MUTEX: Mutex<()> = Mutex::new(());

    struct StateReset(AppState);

    impl Drop for StateReset {
        fn drop(&mut self) {
            let _ = transition_to(self.0);
        }
    }

    #[test]
    fn state_labels_and_recording_capability_match_contract() {
        let _guard = STATE_TEST_MUTEX.lock().expect("state test mutex should lock");

        assert_eq!(AppState::NeedsSetup.as_str(), "needs-setup");
        assert_eq!(AppState::Ready.as_str(), "ready");
        assert_eq!(AppState::Recording.as_str(), "recording");
        assert_eq!(AppState::Processing.as_str(), "processing");

        assert!(!AppState::NeedsSetup.can_record());
        assert!(AppState::Ready.can_record());
        assert!(!AppState::Recording.can_record());
        assert!(!AppState::Processing.can_record());
    }

    #[test]
    fn transition_to_updates_global_state() {
        let _guard = STATE_TEST_MUTEX.lock().expect("state test mutex should lock");
        let _reset = StateReset(get_state());

        transition_to(AppState::Ready).expect("transition to ready should succeed");
        assert_eq!(get_state(), AppState::Ready);

        transition_to(AppState::Recording).expect("transition to recording should succeed");
        assert_eq!(get_state(), AppState::Recording);

        transition_to(AppState::Processing).expect("transition to processing should succeed");
        assert_eq!(get_state(), AppState::Processing);
    }
}
