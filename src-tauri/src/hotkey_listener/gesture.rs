//! Turns hotkey presses and releases into recording actions, so one hotkey
//! supports two ways to record:
//!
//! - Hold to talk: press starts recording, release stops and transcribes.
//! - Hands-free: a quick tap followed by a second press within
//!   [`DOUBLE_TAP_WINDOW`] keeps recording without holding the key; the next
//!   press stops and transcribes.
//!
//! Recording always starts on the first press, so nothing said during the
//! double-tap is lost and holding the key never waits on a timer. Only a
//! quick tap delays its stop, by at most [`DOUBLE_TAP_WINDOW`].
//!
//! Timestamps come from the OS hook, not from when the worker handles an
//! event: opening the microphone blocks the worker, and queued events must
//! still be judged by when the user pressed the key.

use std::time::{Duration, Instant};

use crate::hotkey::RecordingSession;

/// Longest press that still counts as a tap.
const TAP_MAX_HOLD: Duration = Duration::from_millis(300);
/// Time after a tap's release in which a second press enables hands-free.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(350);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Start,
    Stop,
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Held {
        pressed_at: Instant,
    },
    /// A quick tap was released; a second press before `deadline` enables
    /// hands-free, otherwise the recording stops at `deadline`.
    TapReleased {
        deadline: Instant,
    },
    /// Recording continues without holding the key until the next press.
    /// The release of the press that entered this phase does nothing.
    HandsFree,
}

#[derive(Debug, Default)]
pub(super) struct Gesture {
    phase: Phase,
}

impl Gesture {
    /// When the pending tap stops recording unless the key is pressed again.
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::TapReleased { deadline } => Some(deadline),
            _ => None,
        }
    }

    /// Stop a pending tap whose double-tap window ended before `now`.
    pub(super) fn expire(&mut self, now: Instant) -> Option<Action> {
        match self.phase {
            Phase::TapReleased { deadline } if now >= deadline => self.stop(),
            _ => None,
        }
    }

    pub(super) fn press(
        &mut self,
        at: Instant,
        session: impl FnOnce() -> RecordingSession,
    ) -> Option<Action> {
        let session = session();
        // The recording may have ended elsewhere (limit, cancel, rejected
        // start); then this press starts a new one.
        if session == RecordingSession::Inactive {
            return self.start(at);
        }

        match self.phase {
            Phase::Idle => self.start(at),
            Phase::Held { .. } => None,
            // Don't keep a recording going hands-free on a microphone that
            // never opened; stopping reports the failure now.
            Phase::TapReleased { .. } if session == RecordingSession::Failed => self.stop(),
            Phase::TapReleased { .. } => {
                tracing::info!("Hands-free recording");
                self.phase = Phase::HandsFree;
                None
            }
            Phase::HandsFree => self.stop(),
        }
    }

    pub(super) fn release(&mut self, at: Instant) -> Option<Action> {
        let Phase::Held { pressed_at } = self.phase else {
            return None;
        };

        if at.saturating_duration_since(pressed_at) <= TAP_MAX_HOLD {
            self.phase = Phase::TapReleased {
                deadline: at + DOUBLE_TAP_WINDOW,
            };
            return None;
        }

        self.stop()
    }

    /// The system woke from sleep. A recording the key isn't holding
    /// (hands-free, or a tap waiting for its second press) stops, so the
    /// microphone doesn't stay on after wake. A held recording is left to the
    /// release, which the listener's resync reports if it was missed.
    pub(super) fn system_woke(&mut self) -> Option<Action> {
        match self.phase {
            Phase::TapReleased { .. } | Phase::HandsFree => self.stop(),
            Phase::Idle | Phase::Held { .. } => None,
        }
    }

    pub(super) fn cancel(&mut self) -> Action {
        self.phase = Phase::Idle;
        Action::Cancel
    }

    fn start(&mut self, at: Instant) -> Option<Action> {
        self.phase = Phase::Held { pressed_at: at };
        Some(Action::Start)
    }

    fn stop(&mut self) -> Option<Action> {
        self.phase = Phase::Idle;
        Some(Action::Stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(start: Instant, offset: u64) -> Instant {
        start + Duration::from_millis(offset)
    }

    fn live() -> RecordingSession {
        RecordingSession::Live
    }

    #[test]
    fn hold_starts_on_press_and_stops_on_release() {
        let t = Instant::now();
        let mut gesture = Gesture::default();

        assert_eq!(gesture.press(t, live), Some(Action::Start));
        assert_eq!(gesture.release(ms(t, 1200)), Some(Action::Stop));
        assert_eq!(gesture.deadline(), None);
    }

    #[test]
    fn double_tap_enters_hands_free_and_next_press_stops() {
        let t = Instant::now();
        let mut gesture = Gesture::default();

        assert_eq!(gesture.press(t, live), Some(Action::Start));
        assert_eq!(gesture.release(ms(t, 120)), None);
        assert_eq!(gesture.press(ms(t, 250), live), None);
        assert_eq!(gesture.release(ms(t, 330)), None);

        // Hands-free has no deadline; recording runs until the next press.
        assert_eq!(gesture.deadline(), None);
        assert_eq!(gesture.expire(ms(t, 60_000)), None);

        assert_eq!(gesture.press(ms(t, 60_000), live), Some(Action::Stop));
        assert_eq!(gesture.release(ms(t, 60_100)), None);

        // Back to normal: the next press records again.
        assert_eq!(gesture.press(ms(t, 70_000), live), Some(Action::Start));
    }

    #[test]
    fn single_tap_stops_once_the_double_tap_window_ends() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);
        gesture.release(ms(t, 100));

        let deadline = gesture
            .deadline()
            .expect("tap should wait for a second press");
        assert_eq!(gesture.expire(deadline - Duration::from_millis(1)), None);
        assert_eq!(gesture.expire(deadline), Some(Action::Stop));
        assert_eq!(gesture.deadline(), None);
    }

    #[test]
    fn late_second_press_does_not_enter_hands_free() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);
        gesture.release(ms(t, 100));

        // The worker sees the late press before its timeout fires.
        let late = ms(t, 100) + DOUBLE_TAP_WINDOW + Duration::from_millis(1);
        assert_eq!(gesture.expire(late), Some(Action::Stop));
        assert_eq!(gesture.press(late, live), Some(Action::Start));
    }

    #[test]
    fn press_longer_than_a_tap_is_hold_to_talk() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);

        assert_eq!(
            gesture.release(t + TAP_MAX_HOLD + Duration::from_millis(1)),
            Some(Action::Stop)
        );
    }

    #[test]
    fn double_tap_on_failed_microphone_stops_instead_of_going_hands_free() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);
        gesture.release(ms(t, 100));

        assert_eq!(
            gesture.press(ms(t, 200), || RecordingSession::Failed),
            Some(Action::Stop)
        );
        assert_eq!(gesture.release(ms(t, 260)), None);
    }

    #[test]
    fn press_after_hands_free_ended_elsewhere_starts_a_new_recording() {
        // e.g. the recording limit stopped a hands-free recording.
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);
        gesture.release(ms(t, 100));
        gesture.press(ms(t, 200), live);

        assert_eq!(
            gesture.press(ms(t, 400_000), || RecordingSession::Inactive),
            Some(Action::Start)
        );
        assert_eq!(gesture.release(ms(t, 402_000)), Some(Action::Stop));
    }

    #[test]
    fn cancel_resets_from_any_phase() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);
        gesture.release(ms(t, 100));
        gesture.press(ms(t, 200), live);

        assert_eq!(gesture.cancel(), Action::Cancel);
        assert_eq!(gesture.release(ms(t, 260)), None);
        assert_eq!(gesture.press(ms(t, 1000), live), Some(Action::Start));
    }

    #[test]
    fn wake_stops_hands_free_but_not_a_held_recording() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);
        gesture.release(ms(t, 100));
        gesture.press(ms(t, 200), live);
        gesture.release(ms(t, 260));

        assert_eq!(gesture.system_woke(), Some(Action::Stop));
        assert_eq!(gesture.press(ms(t, 9000), live), Some(Action::Start));

        // Still held after wake: keeps recording until the release.
        assert_eq!(gesture.system_woke(), None);
        assert_eq!(gesture.release(ms(t, 12_000)), Some(Action::Stop));
    }

    #[test]
    fn repeated_press_while_held_is_ignored() {
        let t = Instant::now();
        let mut gesture = Gesture::default();
        gesture.press(t, live);

        assert_eq!(gesture.press(ms(t, 50), live), None);
        assert_eq!(gesture.release(ms(t, 1000)), Some(Action::Stop));
    }
}
