use once_cell::sync::Lazy;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    NeedsSetup,
    Initializing,
    Ready,
    Recording,
    Processing,
}

impl AppState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppState::NeedsSetup => "needs-setup",
            AppState::Initializing => "initializing",
            AppState::Ready => "ready",
            AppState::Recording => "recording",
            AppState::Processing => "processing",
        }
    }

    pub fn can_record(&self) -> bool {
        matches!(self, AppState::Ready)
    }

    pub fn is_busy(&self) -> bool {
        matches!(self, AppState::Recording | AppState::Processing)
    }
}

pub static APP_STATE: Lazy<RwLock<AppState>> = Lazy::new(|| RwLock::new(AppState::NeedsSetup));

pub fn transition_to(new_state: AppState) -> Result<(), &'static str> {
    let mut state = APP_STATE.write().unwrap();
    *state = new_state;
    Ok(())
}

/// Set state and emit event to frontend
pub fn set_state(app: &tauri::AppHandle, new_state: AppState) -> Result<(), String> {
    use tauri::Emitter;

    let mut state = APP_STATE.write().unwrap();
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
    *APP_STATE.read().unwrap()
}
