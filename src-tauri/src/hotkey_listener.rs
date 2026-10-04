//! Global hotkey listener.
//!
//! A platform backend (`hotkey_listener/macos.rs`, `hotkey_listener/windows.rs`)
//! owns the OS keyboard hook and reports physical key transitions. This module
//! tracks which keys are held, matches them against the configured hotkey, and
//! supervises the backend so the hotkey keeps working for as long as the app
//! runs: across sleep/hibernate, lock screens, secure input, hook timeouts and
//! permission changes.
//!
//! Robustness invariants:
//! - Held-key state self-corrects: before matching, keys the OS no longer
//!   reports as down are dropped, so a lost key-up can never block the hotkey.
//! - The backend runs under a supervisor that restarts it whenever it exits.
//! - Backends re-enable / reinstall their hook after wake and on a schedule.
//!
//! Escape is never part of a hotkey; while the hotkey is held it cancels the
//! recording instead of stopping it, so nothing is transcribed or pasted.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::hotkey_config::{get_hotkey_config, HotkeyConfig};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as backend;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use once_cell::sync::Lazy;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::collections::HashMap;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::time::{Duration, Instant};

static LISTENER_STARTED: AtomicBool = AtomicBool::new(false);
static SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Delay before the first restart attempt after a backend failure.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const MIN_RESTART_DELAY: Duration = Duration::from_secs(1);
/// Upper bound for the exponential restart backoff.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
/// A backend run at least this long counts as healthy and resets the backoff.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const MIN_HEALTHY_RUN: Duration = Duration::from_secs(10);
/// How often to re-check a missing permission before creating the hook.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const PERMISSION_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Time spent asleep between two health checks that counts as a wake.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const SLEEP_DETECTION_THRESHOLD_NS: u64 = 2_000_000_000;

/// Suppress global hotkey activation (e.g. while the rebind modal is open).
/// Pass-through still occurs so the modal can capture the keys; the listener
/// just stops dispatching press/release events and clears any in-flight state.
pub fn set_suppressed(suppressed: bool) {
    SUPPRESSED.store(suppressed, Ordering::SeqCst);
    if suppressed {
        reset_listener_state();
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
#[cfg(any(target_os = "macos", target_os = "windows"))]
static HOTKEY_STATE: Lazy<Mutex<HotkeyState>> = Lazy::new(|| Mutex::new(HotkeyState::default()));
#[cfg(any(target_os = "macos", target_os = "windows"))]
static HOTKEY_EVENT_TX: OnceLock<mpsc::Sender<HotkeyEvent>> = OnceLock::new();

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HotkeyEvent {
    Press,
    Release,
    Cancel,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) use crate::hotkey_config::ESCAPE_TOKEN;

/// A physical key transition reported by a platform backend.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyEvent {
    pub token: &'static str,
    /// Platform key code (macOS virtual keycode / Windows virtual-key code),
    /// used to ask the OS whether the key is still physically held.
    pub code: u32,
    pub is_press: bool,
}

pub fn start_hotkey_listener(app: AppHandle) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        start_hotkey_listener_impl(app)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        tracing::warn!("Global hotkey not implemented for this platform");
        let _ = app;
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_hotkey_listener_impl(app: AppHandle) -> Result<(), String> {
    let _ = APP_HANDLE.set(app);

    HOTKEY_EVENT_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<HotkeyEvent>();

        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                let Some(app) = APP_HANDLE.get() else {
                    continue;
                };

                run_hotkey_event(app, event);
            }
        });

        tx
    });

    if !LISTENER_STARTED.swap(true, Ordering::SeqCst) {
        // The supervisor waits for permission itself, so the hotkey starts
        // working as soon as it is granted, without an app restart.
        if let Err(error) = std::thread::Builder::new()
            .name("fing-hotkey-listener".to_string())
            .spawn(supervise_backend)
        {
            LISTENER_STARTED.store(false, Ordering::SeqCst);
            return Err(format!("Failed to start hotkey listener thread: {error}"));
        }
    }

    if !backend::is_permitted() {
        return Err("Accessibility permission required for global hotkey".to_string());
    }

    Ok(())
}

/// Run the platform backend forever, restarting it whenever it exits.
/// `backend::run` blocks while the hook is healthy; `Ok` means an intentional
/// restart (e.g. after wake), `Err` means the hook failed or was lost.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn supervise_backend() {
    let mut restart_delay = MIN_RESTART_DELAY;
    let mut waiting_for_permission = false;

    loop {
        if !backend::is_permitted() {
            if !waiting_for_permission {
                tracing::info!("Hotkey listener waiting for Accessibility permission");
                waiting_for_permission = true;
            }
            std::thread::sleep(PERMISSION_POLL_INTERVAL);
            continue;
        }
        waiting_for_permission = false;

        let started_at = Instant::now();
        let result = backend::run();
        let healthy = started_at.elapsed() >= MIN_HEALTHY_RUN;
        if healthy {
            restart_delay = MIN_RESTART_DELAY;
        }

        match result {
            Ok(reason) if healthy => {
                tracing::info!("Restarting hotkey listener: {}", reason);
                continue;
            }
            Ok(reason) => tracing::warn!(
                "Hotkey listener stopped early ({}); restarting in {}s",
                reason,
                restart_delay.as_secs()
            ),
            Err(error) => tracing::error!(
                "Hotkey listener failed: {}; restarting in {}s",
                error,
                restart_delay.as_secs()
            ),
        }

        std::thread::sleep(restart_delay);
        restart_delay = (restart_delay * 2).min(MAX_RESTART_DELAY);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run_hotkey_event(app: &AppHandle, event: HotkeyEvent) {
    match event {
        HotkeyEvent::Press => crate::hotkey::on_key_down(app),
        HotkeyEvent::Release => crate::hotkey::on_key_up(app),
        HotkeyEvent::Cancel => crate::hotkey::on_key_cancel(app),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn dispatch_hotkey_event(event: HotkeyEvent) {
    let Some(tx) = HOTKEY_EVENT_TX.get() else {
        if let Some(app) = APP_HANDLE.get() {
            run_hotkey_event(app, event);
        }
        return;
    };

    if let Err(error) = tx.send(event) {
        tracing::warn!("Hotkey worker unavailable: {}", error);
        if let Some(app) = APP_HANDLE.get() {
            run_hotkey_event(app, event);
        }
    }
}

/// Onboarding test mode lets the macOS tap fire before setup completes. On
/// Windows the onboarding test runs through the focused WebView's key
/// handlers, so the global hook keeps ignoring presses until `Ready`.
#[cfg(target_os = "macos")]
fn onboarding_test_mode() -> bool {
    crate::hotkey::is_onboarding_test_mode()
}

#[cfg(target_os = "windows")]
fn onboarding_test_mode() -> bool {
    false
}

/// Handle one key transition from the backend's hook callback.
/// Returns `true` when the event should be swallowed; backends that can't
/// block events ignore it.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn handle_key_event(event: KeyEvent) -> bool {
    if SUPPRESSED.load(Ordering::SeqCst) {
        return false;
    }

    let Some(config) = get_hotkey_config() else {
        return false;
    };

    let test_mode = onboarding_test_mode();
    let outcome = lock_hotkey_state().on_event(event, &config, backend::is_key_down, || {
        test_mode || crate::state::get_state().can_record()
    });

    if let Some(hotkey_event) = outcome.dispatch {
        dispatch_hotkey_event(hotkey_event);
    }

    outcome.swallow && !test_mode
}

/// Called by a backend when it detects that the system woke from sleep.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn handle_system_wake() {
    tracing::info!("System wake detected, resyncing hotkey listener");
    resync_listener_state();
}

/// Detects system sleep by comparing a clock that runs during sleep with
/// one that doesn't.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) struct SleepWatch {
    last_asleep_ns: u64,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SleepWatch {
    pub(crate) fn new() -> Self {
        Self {
            last_asleep_ns: backend::asleep_ns(),
        }
    }

    /// Whether the system slept since the previous call.
    pub(crate) fn system_slept(&mut self) -> bool {
        let now = backend::asleep_ns();
        let slept = slept_between(self.last_asleep_ns, now);
        self.last_asleep_ns = now;
        slept
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn slept_between(previous_asleep_ns: u64, current_asleep_ns: u64) -> bool {
    current_asleep_ns.saturating_sub(previous_asleep_ns) >= SLEEP_DETECTION_THRESHOLD_NS
}

/// Whether no keys are tracked as held, so the hook can be swapped safely.
#[cfg(target_os = "windows")]
pub(crate) fn is_idle() -> bool {
    let state = lock_hotkey_state();
    !state.hotkey_active && state.pressed_keys.is_empty()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug, Default, PartialEq, Eq)]
struct EventOutcome {
    dispatch: Option<HotkeyEvent>,
    swallow: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl EventOutcome {
    fn swallowed(dispatch: Option<HotkeyEvent>) -> Self {
        Self {
            dispatch,
            swallow: true,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct HotkeyState {
    hotkey_active: bool,
    /// Held keys by token, with the platform code used to re-check them.
    pressed_keys: HashMap<&'static str, u32>,
    /// Code of the Escape key that cancelled a recording while it is still
    /// held: its repeats and key-up are swallowed so they don't reach the
    /// focused app.
    swallowed_escape: Option<u32>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl HotkeyState {
    fn on_event(
        &mut self,
        event: KeyEvent,
        config: &HotkeyConfig,
        is_key_down: impl Fn(u32) -> bool,
        can_activate: impl FnOnce() -> bool,
    ) -> EventOutcome {
        if event.token == ESCAPE_TOKEN {
            return self.on_escape(event);
        }

        if event.is_press {
            // The OS can drop key-ups (lock screen, secure input, elevated
            // windows, sleep). Drop keys that are no longer physically held
            // so they can't block the exact-match below forever.
            let stale_release = self.prune_released_keys(Some(event.token), config, &is_key_down);
            self.pressed_keys.insert(event.token, event.code);

            if stale_release {
                return EventOutcome {
                    dispatch: Some(HotkeyEvent::Release),
                    swallow: false,
                };
            }

            if self.hotkey_active {
                return EventOutcome::swallowed(None);
            }

            if hotkey_matches(&self.pressed_keys, config) && can_activate() {
                self.hotkey_active = true;
                return EventOutcome::swallowed(Some(HotkeyEvent::Press));
            }

            return EventOutcome::default();
        }

        self.pressed_keys.remove(event.token);

        if !self.hotkey_active {
            return EventOutcome::default();
        }

        let dispatch = if config.key_set.contains(event.token) {
            self.hotkey_active = false;
            Some(HotkeyEvent::Release)
        } else {
            None
        };

        EventOutcome::swallowed(dispatch)
    }

    /// Escape is never tracked as a held key. Pressed while the hotkey is
    /// active, it ends the hotkey without a release so the recording is
    /// discarded; the remaining hotkey key-ups then dispatch nothing.
    fn on_escape(&mut self, event: KeyEvent) -> EventOutcome {
        if !event.is_press {
            return EventOutcome {
                dispatch: None,
                swallow: self.swallowed_escape.take().is_some(),
            };
        }

        if self.hotkey_active {
            self.hotkey_active = false;
            self.swallowed_escape = Some(event.code);
            return EventOutcome::swallowed(Some(HotkeyEvent::Cancel));
        }

        EventOutcome {
            dispatch: None,
            swallow: self.swallowed_escape.is_some(),
        }
    }

    /// Drop tracked keys the OS reports as released (except `current`, whose
    /// state isn't updated yet inside the hook). Returns `true` when an active
    /// hotkey is no longer physically held, i.e. its release was missed.
    fn prune_released_keys(
        &mut self,
        current: Option<&str>,
        config: &HotkeyConfig,
        is_key_down: impl Fn(u32) -> bool,
    ) -> bool {
        let tracked = self.pressed_keys.len();
        self.pressed_keys
            .retain(|token, code| Some(*token) == current || is_key_down(*code));

        let pruned = tracked - self.pressed_keys.len();
        if pruned > 0 {
            tracing::info!("Dropped {} stale held key(s) from hotkey state", pruned);
        }
        if self.swallowed_escape.is_some_and(|code| !is_key_down(code)) {
            self.swallowed_escape = None;
        }

        if !self.hotkey_active {
            return false;
        }

        let still_held = config
            .key_set
            .iter()
            .all(|token| self.pressed_keys.contains_key(token.as_str()));
        if still_held {
            return false;
        }

        self.hotkey_active = false;
        true
    }

    /// Bring the tracked state back in line with the keys the OS reports as
    /// held. Returns `true` when an active hotkey was released meanwhile.
    fn resync(&mut self, config: Option<&HotkeyConfig>, is_key_down: impl Fn(u32) -> bool) -> bool {
        match config {
            Some(config) => self.prune_released_keys(None, config, is_key_down),
            None => {
                *self = Self::default();
                false
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn lock_hotkey_state() -> MutexGuard<'static, HotkeyState> {
    match HOTKEY_STATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn reset_listener_state() {
    *lock_hotkey_state() = HotkeyState::default();
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn reset_listener_state() {}

/// Re-check tracked keys against the OS after events may have been missed
/// (wake, hook restart, tap re-enable). Unlike `reset_listener_state`, keys
/// that are still held stay tracked, and an active hotkey whose release was
/// missed is released so recording stops.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn resync_listener_state() {
    let config = get_hotkey_config();
    let release = lock_hotkey_state().resync(config.as_ref(), backend::is_key_down);
    if release {
        tracing::info!("Hotkey release was missed; stopping recording");
        dispatch_hotkey_event(HotkeyEvent::Release);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn resync_listener_state() {}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn config_contains_function_free_f_key(config: &HotkeyConfig) -> bool {
    !config.key_set.contains("Function")
        && config
            .key_set
            .iter()
            .any(|token| token.starts_with('F') && token[1..].parse::<u8>().is_ok())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn hotkey_matches(pressed_keys: &HashMap<&'static str, u32>, config: &HotkeyConfig) -> bool {
    let ignores_function =
        config_contains_function_free_f_key(config) && pressed_keys.contains_key("Function");
    let pressed_count = pressed_keys.len() - usize::from(ignores_function);

    pressed_count == config.key_set.len()
        && config
            .key_set
            .iter()
            .all(|token| pressed_keys.contains_key(token.as_str()))
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn config(keys: &[&str]) -> HotkeyConfig {
        HotkeyConfig {
            key_set: keys
                .iter()
                .map(|key| key.to_string())
                .collect::<HashSet<_>>(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
        }
    }

    /// Assigns each token a stable fake platform code.
    fn code(token: &str) -> u32 {
        token.bytes().fold(0u32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u32::from(byte))
        })
    }

    fn pressed(keys: &[&'static str]) -> HashMap<&'static str, u32> {
        keys.iter().map(|key| (*key, code(key))).collect()
    }

    fn press(token: &'static str) -> KeyEvent {
        KeyEvent {
            token,
            code: code(token),
            is_press: true,
        }
    }

    fn release(token: &'static str) -> KeyEvent {
        KeyEvent {
            token,
            code: code(token),
            is_press: false,
        }
    }

    /// OS key-state stub reporting only `held` keys as down.
    fn os_holding(held: &'static [&'static str]) -> impl Fn(u32) -> bool {
        move |key_code| held.iter().any(|token| code(token) == key_code)
    }

    #[test]
    fn matches_exact_key_sets() {
        let config = config(&["ControlLeft", "KeyK"]);

        assert!(hotkey_matches(&pressed(&["ControlLeft", "KeyK"]), &config));
        assert!(!hotkey_matches(&pressed(&["ControlLeft"]), &config));
        assert!(!hotkey_matches(
            &pressed(&["ControlLeft", "KeyK", "Space"]),
            &config
        ));
    }

    #[test]
    fn ignores_function_for_function_key_hotkeys() {
        let config = config(&["F9"]);

        assert!(hotkey_matches(&pressed(&["F9"]), &config));
        assert!(hotkey_matches(&pressed(&["Function", "F9"]), &config));
    }

    #[test]
    fn requires_function_when_configured() {
        let config = config(&["Function", "F9"]);

        assert!(hotkey_matches(&pressed(&["Function", "F9"]), &config));
        assert!(!hotkey_matches(&pressed(&["F9"]), &config));
    }

    #[test]
    fn press_and_release_dispatch_hotkey_events() {
        let config = config(&["F9"]);
        let mut state = HotkeyState::default();

        let outcome = state.on_event(press("F9"), &config, os_holding(&[]), || true);
        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Press));
        assert!(outcome.swallow);

        let outcome = state.on_event(release("F9"), &config, os_holding(&[]), || true);
        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Release));
        assert!(state.pressed_keys.is_empty());
    }

    #[test]
    fn does_not_activate_when_app_cannot_record() {
        let config = config(&["F9"]);
        let mut state = HotkeyState::default();

        let outcome = state.on_event(press("F9"), &config, os_holding(&[]), || false);
        assert_eq!(outcome, EventOutcome::default());
        assert!(!state.hotkey_active);
    }

    #[test]
    fn missed_key_ups_do_not_block_the_hotkey() {
        // Win+L / Ctrl+Cmd+Q: the lock screen swallows both key-ups.
        let config = config(&["F9"]);
        let mut state = HotkeyState::default();
        state.on_event(press("MetaLeft"), &config, os_holding(&[]), || true);
        state.on_event(press("KeyL"), &config, os_holding(&["MetaLeft"]), || true);

        let outcome = state.on_event(press("F9"), &config, os_holding(&[]), || true);

        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Press));
        assert_eq!(state.pressed_keys, pressed(&["F9"]));
    }

    #[test]
    fn keeps_physically_held_modifiers() {
        let config = config(&["ControlLeft", "Space"]);
        let mut state = HotkeyState::default();
        state.on_event(press("ControlLeft"), &config, os_holding(&[]), || true);

        let outcome = state.on_event(
            press("Space"),
            &config,
            os_holding(&["ControlLeft"]),
            || true,
        );

        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Press));
    }

    #[test]
    fn missed_hotkey_release_stops_recording_on_next_press() {
        let config = config(&["F9"]);
        let mut state = HotkeyState::default();
        state.on_event(press("F9"), &config, os_holding(&[]), || true);

        // F9's key-up was lost; the next key press must not be swallowed.
        let outcome = state.on_event(press("KeyA"), &config, os_holding(&[]), || true);

        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Release));
        assert!(!outcome.swallow);
        assert!(!state.hotkey_active);
    }

    #[test]
    fn other_keys_are_swallowed_while_hotkey_is_held() {
        let config = config(&["F9"]);
        let mut state = HotkeyState::default();
        state.on_event(press("F9"), &config, os_holding(&[]), || true);

        let outcome = state.on_event(press("KeyA"), &config, os_holding(&["F9"]), || true);

        assert_eq!(outcome.dispatch, None);
        assert!(outcome.swallow);
        assert!(state.hotkey_active);
    }

    #[test]
    fn escape_cancels_active_hotkey_and_is_swallowed() {
        let config = config(&["MetaRight"]);
        let mut state = HotkeyState::default();
        state.on_event(press("MetaRight"), &config, os_holding(&[]), || true);

        let outcome = state.on_event(
            press(ESCAPE_TOKEN),
            &config,
            os_holding(&["MetaRight"]),
            || true,
        );
        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Cancel));
        assert!(outcome.swallow);
        assert!(!state.hotkey_active);

        // Auto-repeat and key-up of the cancelling Escape are swallowed too.
        let outcome = state.on_event(
            press(ESCAPE_TOKEN),
            &config,
            os_holding(&["MetaRight"]),
            || true,
        );
        assert_eq!(outcome.dispatch, None);
        assert!(outcome.swallow);
        let outcome = state.on_event(release(ESCAPE_TOKEN), &config, os_holding(&[]), || true);
        assert_eq!(outcome.dispatch, None);
        assert!(outcome.swallow);

        // Releasing the hotkey afterwards must not start a transcription.
        let outcome = state.on_event(release("MetaRight"), &config, os_holding(&[]), || true);
        assert_eq!(outcome, EventOutcome::default());
        assert!(state.pressed_keys.is_empty());

        // The next hotkey press records again.
        let outcome = state.on_event(press("MetaRight"), &config, os_holding(&[]), || true);
        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Press));
    }

    #[test]
    fn missed_escape_release_stops_swallowing_escape() {
        let config = config(&["MetaRight"]);
        let mut state = HotkeyState::default();
        state.on_event(press("MetaRight"), &config, os_holding(&[]), || true);
        state.on_event(
            press(ESCAPE_TOKEN),
            &config,
            os_holding(&["MetaRight"]),
            || true,
        );

        // Escape's key-up was lost; the next resync sees it is no longer held.
        state.resync(Some(&config), os_holding(&[]));

        let outcome = state.on_event(press(ESCAPE_TOKEN), &config, os_holding(&[]), || true);
        assert_eq!(outcome, EventOutcome::default());
    }

    #[test]
    fn escape_passes_through_when_hotkey_is_not_active() {
        let config = config(&["MetaRight"]);
        let mut state = HotkeyState::default();

        let outcome = state.on_event(press(ESCAPE_TOKEN), &config, os_holding(&[]), || true);
        assert_eq!(outcome, EventOutcome::default());
        let outcome = state.on_event(release(ESCAPE_TOKEN), &config, os_holding(&[]), || true);
        assert_eq!(outcome, EventOutcome::default());
        assert!(state.pressed_keys.is_empty());
    }

    #[test]
    fn held_escape_does_not_block_the_hotkey() {
        let config = config(&["MetaRight"]);
        let mut state = HotkeyState::default();
        state.on_event(press(ESCAPE_TOKEN), &config, os_holding(&[]), || true);

        let outcome = state.on_event(
            press("MetaRight"),
            &config,
            os_holding(&[ESCAPE_TOKEN]),
            || true,
        );
        assert_eq!(outcome.dispatch, Some(HotkeyEvent::Press));
    }

    #[test]
    fn resync_releases_hotkey_only_when_no_longer_held() {
        let config = config(&["ControlLeft", "Space"]);
        let mut state = HotkeyState::default();
        state.on_event(press("ControlLeft"), &config, os_holding(&[]), || true);
        state.on_event(
            press("Space"),
            &config,
            os_holding(&["ControlLeft"]),
            || true,
        );

        assert!(!state.resync(Some(&config), os_holding(&["ControlLeft", "Space"])));
        assert!(state.hotkey_active);

        assert!(state.resync(Some(&config), os_holding(&["ControlLeft"])));
        assert!(!state.hotkey_active);
        assert_eq!(state.pressed_keys, pressed(&["ControlLeft"]));
    }

    #[test]
    fn resync_without_config_clears_state() {
        let config = config(&["F9"]);
        let mut state = HotkeyState::default();
        state.on_event(press("F9"), &config, os_holding(&[]), || true);

        assert!(!state.resync(None, os_holding(&["F9"])));
        assert!(!state.hotkey_active);
        assert!(state.pressed_keys.is_empty());
    }

    /// Every token a backend can report must be one the settings accept,
    /// otherwise a saved hotkey could never match.
    pub(crate) fn assert_keymap_is_consistent<C: Copy + Eq + std::hash::Hash + std::fmt::Debug>(
        keymap: &[(C, &str)],
    ) {
        let mut codes = HashSet::new();
        let mut tokens = HashSet::new();
        for (code, token) in keymap {
            // Escape is mapped so it can cancel a recording, but is never a hotkey.
            assert!(
                *token == ESCAPE_TOKEN || crate::hotkey_config::parse_hotkey_string(token).is_ok(),
                "keymap token {token} is not a valid hotkey token"
            );
            assert!(codes.insert(*code), "duplicate keycode {code:?}");
            assert!(tokens.insert(*token), "duplicate keymap token {token}");
        }
    }
}
