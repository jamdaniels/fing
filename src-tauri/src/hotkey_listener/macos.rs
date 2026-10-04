//! macOS backend: a session-level `CGEventTap` on the listener thread's run loop.
//!
//! The tap only receives keyboard events (no mouse traffic) and does no
//! character translation, so its callback stays fast. macOS disables taps
//! whose callback is too slow; that is handled in the callback itself and by a
//! periodic health check, which also recreates the tap after wake and stops it
//! if Accessibility permission is revoked.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use super::{handle_key_event, KeyEvent, SleepWatch, ESCAPE_TOKEN};

type CFMachPortRef = *mut c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFStringRef = *const c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type TapCallback =
    unsafe extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

#[repr(C)]
#[derive(Default)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: CFMachPortRef,
        order: isize,
    ) -> CFRunLoopSourceRef;
    fn CFMachPortIsValid(port: CFMachPortRef) -> u8;
    fn CFMachPortInvalidate(port: CFMachPortRef);
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source_handled: u8) -> i32;
    fn CFRelease(cf: *const c_void);
    static kCFRunLoopDefaultMode: CFStringRef;
}

extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_continuous_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

const SESSION_EVENT_TAP: u32 = 1;
const HEAD_INSERT_EVENT_TAP: u32 = 0;
const TAP_OPTION_DEFAULT: u32 = 0;

const EVENT_KEY_DOWN: u32 = 10;
const EVENT_KEY_UP: u32 = 11;
const EVENT_FLAGS_CHANGED: u32 = 12;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const KEY_EVENT_MASK: u64 =
    (1 << EVENT_KEY_DOWN) | (1 << EVENT_KEY_UP) | (1 << EVENT_FLAGS_CHANGED);

const FIELD_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const FIELD_EVENT_SOURCE_UNIX_PROCESS_ID: u32 = 41;
const SOURCE_STATE_COMBINED_SESSION: i32 = 0;
const RUN_LOOP_FINISHED: i32 = 1;

// CGEventFlags: device-independent modifier masks.
const FLAG_ALPHA_SHIFT: u64 = 0x0001_0000;
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_ALTERNATE: u64 = 0x0008_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;
const FLAG_SECONDARY_FN: u64 = 0x0080_0000;
// IOKit NX_DEVICE* masks: which side of a modifier pair is down.
const DEVICE_LEFT_CONTROL: u64 = 0x0000_0001;
const DEVICE_LEFT_SHIFT: u64 = 0x0000_0002;
const DEVICE_RIGHT_SHIFT: u64 = 0x0000_0004;
const DEVICE_LEFT_COMMAND: u64 = 0x0000_0008;
const DEVICE_RIGHT_COMMAND: u64 = 0x0000_0010;
const DEVICE_LEFT_ALTERNATE: u64 = 0x0000_0020;
const DEVICE_RIGHT_ALTERNATE: u64 = 0x0000_0040;
const DEVICE_RIGHT_CONTROL: u64 = 0x0000_2000;

const KVK_RIGHT_COMMAND: u16 = 54;
const KVK_COMMAND: u16 = 55;
const KVK_SHIFT: u16 = 56;
const KVK_CAPS_LOCK: u16 = 57;
const KVK_OPTION: u16 = 58;
const KVK_CONTROL: u16 = 59;
const KVK_RIGHT_SHIFT: u16 = 60;
const KVK_RIGHT_OPTION: u16 = 61;
const KVK_RIGHT_CONTROL: u16 = 62;
const KVK_FUNCTION: u16 = 63;

/// How often the run loop wakes up to check the tap's health.
const HEALTH_CHECK_INTERVAL_SECS: f64 = 5.0;

/// The live tap, so the callback can re-enable it when macOS disables it.
static TAP: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

/// macOS virtual keycodes (Carbon `kVK_*`) to hotkey tokens, plus Escape,
/// which cancels a recording but can't be part of a hotkey.
#[rustfmt::skip]
const KEYMAP: &[(u16, &str)] = &[
    (0, "KeyA"), (1, "KeyS"), (2, "KeyD"), (3, "KeyF"), (4, "KeyH"), (5, "KeyG"),
    (6, "KeyZ"), (7, "KeyX"), (8, "KeyC"), (9, "KeyV"), (10, "IntlBackslash"),
    (11, "KeyB"), (12, "KeyQ"), (13, "KeyW"), (14, "KeyE"), (15, "KeyR"),
    (16, "KeyY"), (17, "KeyT"), (18, "Num1"), (19, "Num2"), (20, "Num3"),
    (21, "Num4"), (22, "Num6"), (23, "Num5"), (24, "Equal"), (25, "Num9"),
    (26, "Num7"), (27, "Minus"), (28, "Num8"), (29, "Num0"), (30, "RightBracket"),
    (31, "KeyO"), (32, "KeyU"), (33, "LeftBracket"), (34, "KeyI"), (35, "KeyP"),
    (36, "Return"), (37, "KeyL"), (38, "KeyJ"), (39, "Quote"), (40, "KeyK"),
    (41, "SemiColon"), (42, "Backslash"), (43, "Comma"), (44, "Slash"),
    (45, "KeyN"), (46, "KeyM"), (47, "Dot"), (48, "Tab"), (49, "Space"),
    (50, "BackQuote"), (51, "Backspace"), (53, ESCAPE_TOKEN), (54, "MetaRight"), (55, "MetaLeft"),
    (56, "ShiftLeft"), (57, "CapsLock"), (58, "Alt"), (59, "ControlLeft"),
    (60, "ShiftRight"), (61, "AltGr"), (62, "ControlRight"), (63, "Function"),
    (64, "F17"), (65, "KpDecimal"), (67, "KpMultiply"), (69, "KpPlus"),
    (71, "NumLock"), (72, "VolumeUp"), (73, "VolumeDown"), (74, "VolumeMute"),
    (75, "KpDivide"), (76, "KpReturn"), (78, "KpMinus"), (79, "F18"), (80, "F19"),
    (81, "KpEqual"), (82, "Kp0"), (83, "Kp1"), (84, "Kp2"), (85, "Kp3"),
    (86, "Kp4"), (87, "Kp5"), (88, "Kp6"), (89, "Kp7"), (90, "F20"), (91, "Kp8"),
    (92, "Kp9"), (93, "IntlYen"), (94, "IntlRo"), (95, "KpComma"), (96, "F5"),
    (97, "F6"), (98, "F7"), (99, "F3"), (100, "F8"), (101, "F9"), (102, "Lang2"),
    (103, "F11"), (104, "Lang1"), (105, "F13"), (106, "F16"), (107, "F14"),
    (109, "F10"), (110, "Apps"), (111, "F12"), (113, "F15"), (114, "Insert"),
    (115, "Home"), (116, "PageUp"), (117, "Delete"), (118, "F4"), (119, "End"),
    (120, "F2"), (121, "PageDown"), (122, "F1"), (123, "LeftArrow"),
    (124, "RightArrow"), (125, "DownArrow"), (126, "UpArrow"),
];

fn token_for_code(code: u16) -> Option<&'static str> {
    KEYMAP
        .iter()
        .find(|(keycode, _)| *keycode == code)
        .map(|(_, token)| *token)
}

/// The event tap needs Accessibility permission. Creating it earlier would
/// only fail (and may trigger an Input Monitoring prompt), so wait for it.
pub(super) fn is_permitted() -> bool {
    crate::platform::check_accessibility_permission()
}

/// Whether the OS currently reports the key as held.
pub(super) fn is_key_down(code: u32) -> bool {
    let Ok(code) = u16::try_from(code) else {
        return false;
    };

    unsafe {
        if code == KVK_FUNCTION {
            return CGEventSourceFlagsState(SOURCE_STATE_COMBINED_SESSION) & FLAG_SECONDARY_FN != 0;
        }
        CGEventSourceKeyState(SOURCE_STATE_COMBINED_SESSION, code)
    }
}

/// Total time the system has spent asleep since boot, in nanoseconds.
pub(super) fn asleep_ns() -> u64 {
    static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo::default();
        if unsafe { mach_timebase_info(&mut info) } != 0 || info.denom == 0 {
            return (1, 1);
        }
        (u64::from(info.numer), u64::from(info.denom))
    });

    // mach_continuous_time keeps counting during sleep; mach_absolute_time doesn't.
    let asleep_ticks = unsafe { mach_continuous_time().saturating_sub(mach_absolute_time()) };
    (u128::from(asleep_ticks) * u128::from(numer) / u128::from(denom)) as u64
}

/// Create the tap and run it until it needs to be recreated.
pub(super) fn run() -> Result<&'static str, String> {
    unsafe {
        let tap = CGEventTapCreate(
            SESSION_EVENT_TAP,
            HEAD_INSERT_EVENT_TAP,
            TAP_OPTION_DEFAULT,
            KEY_EVENT_MASK,
            tap_callback,
            ptr::null_mut(),
        );
        if tap.is_null() {
            return Err("CGEventTapCreate failed".to_string());
        }

        let source = CFMachPortCreateRunLoopSource(ptr::null(), tap, 0);
        if source.is_null() {
            CFMachPortInvalidate(tap);
            CFRelease(tap);
            return Err("CFMachPortCreateRunLoopSource failed".to_string());
        }

        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        TAP.store(tap, Ordering::SeqCst);
        CGEventTapEnable(tap, true);
        tracing::info!("Hotkey event tap started");

        // Events may have been missed while no tap was active.
        super::resync_listener_state();
        let result = monitor_tap(tap);

        TAP.store(ptr::null_mut(), Ordering::SeqCst);
        CGEventTapEnable(tap, false);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
        CFMachPortInvalidate(tap);
        CFRelease(source);
        CFRelease(tap);

        result
    }
}

/// Service the tap, checking its health every few seconds.
unsafe fn monitor_tap(tap: CFMachPortRef) -> Result<&'static str, String> {
    let mut sleep_watch = SleepWatch::new();

    loop {
        let result = CFRunLoopRunInMode(kCFRunLoopDefaultMode, HEALTH_CHECK_INTERVAL_SECS, 0);
        if result == RUN_LOOP_FINISHED {
            return Err("event tap run loop has no sources".to_string());
        }

        if sleep_watch.system_slept() {
            super::handle_system_wake();
            return Ok("system woke from sleep");
        }

        if !is_permitted() {
            return Err("Accessibility permission was revoked".to_string());
        }

        if CFMachPortIsValid(tap) == 0 {
            return Err("event tap was invalidated".to_string());
        }

        if !CGEventTapIsEnabled(tap) {
            tracing::warn!("Hotkey event tap was disabled, re-enabling");
            CGEventTapEnable(tap, true);
            super::resync_listener_state();
        }
    }
}

unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    _user_info: *mut c_void,
) -> CGEventRef {
    match event_type {
        EVENT_TAP_DISABLED_BY_TIMEOUT | EVENT_TAP_DISABLED_BY_USER_INPUT => {
            let tap = TAP.load(Ordering::SeqCst);
            if !tap.is_null() {
                CGEventTapEnable(tap, true);
            }
            let cause = if event_type == EVENT_TAP_DISABLED_BY_TIMEOUT {
                "timeout"
            } else {
                "user input"
            };
            tracing::warn!("Hotkey event tap disabled by {}, re-enabled", cause);
            // Key events delivered while disabled were missed.
            super::resync_listener_state();
            event
        }
        EVENT_KEY_DOWN | EVENT_KEY_UP | EVENT_FLAGS_CHANGED => {
            let Some(key_event) = convert(event_type, event) else {
                return event;
            };
            if handle_key_event(key_event) {
                // Returning NULL from an active tap deletes the event.
                ptr::null_mut()
            } else {
                event
            }
        }
        _ => event,
    }
}

unsafe fn convert(event_type: u32, event: CGEventRef) -> Option<KeyEvent> {
    // Our own paste (enigo) posts keycode-0 key-downs without key-ups, and
    // macOS then reports that key as held, so it would block the hotkey.
    if CGEventGetIntegerValueField(event, FIELD_EVENT_SOURCE_UNIX_PROCESS_ID)
        == i64::from(std::process::id())
    {
        return None;
    }

    let code = u16::try_from(CGEventGetIntegerValueField(
        event,
        FIELD_KEYBOARD_EVENT_KEYCODE,
    ))
    .ok()?;
    let token = token_for_code(code)?;
    let is_press = match event_type {
        EVENT_KEY_DOWN => true,
        EVENT_KEY_UP => false,
        _ => modifier_is_down(code, CGEventGetFlags(event))?,
    };

    Some(KeyEvent {
        token,
        code: u32::from(code),
        is_press,
    })
}

/// Whether a `FlagsChanged` event for `code` is a press, read from that
/// modifier's own flag bit rather than guessed from the previous flags.
fn modifier_is_down(code: u16, flags: u64) -> Option<bool> {
    let (side_mask, pair_mask, modifier_mask) = match code {
        KVK_SHIFT => (
            DEVICE_LEFT_SHIFT,
            DEVICE_LEFT_SHIFT | DEVICE_RIGHT_SHIFT,
            FLAG_SHIFT,
        ),
        KVK_RIGHT_SHIFT => (
            DEVICE_RIGHT_SHIFT,
            DEVICE_LEFT_SHIFT | DEVICE_RIGHT_SHIFT,
            FLAG_SHIFT,
        ),
        KVK_CONTROL => (
            DEVICE_LEFT_CONTROL,
            DEVICE_LEFT_CONTROL | DEVICE_RIGHT_CONTROL,
            FLAG_CONTROL,
        ),
        KVK_RIGHT_CONTROL => (
            DEVICE_RIGHT_CONTROL,
            DEVICE_LEFT_CONTROL | DEVICE_RIGHT_CONTROL,
            FLAG_CONTROL,
        ),
        KVK_OPTION => (
            DEVICE_LEFT_ALTERNATE,
            DEVICE_LEFT_ALTERNATE | DEVICE_RIGHT_ALTERNATE,
            FLAG_ALTERNATE,
        ),
        KVK_RIGHT_OPTION => (
            DEVICE_RIGHT_ALTERNATE,
            DEVICE_LEFT_ALTERNATE | DEVICE_RIGHT_ALTERNATE,
            FLAG_ALTERNATE,
        ),
        KVK_COMMAND => (
            DEVICE_LEFT_COMMAND,
            DEVICE_LEFT_COMMAND | DEVICE_RIGHT_COMMAND,
            FLAG_COMMAND,
        ),
        KVK_RIGHT_COMMAND => (
            DEVICE_RIGHT_COMMAND,
            DEVICE_LEFT_COMMAND | DEVICE_RIGHT_COMMAND,
            FLAG_COMMAND,
        ),
        KVK_FUNCTION => return Some(flags & FLAG_SECONDARY_FN != 0),
        // Caps Lock only reports toggles: "press" when it turns on.
        KVK_CAPS_LOCK => return Some(flags & FLAG_ALPHA_SHIFT != 0),
        _ => return None,
    };

    // Some virtual keyboards omit the per-side bits; fall back to the
    // side-independent modifier flag.
    if flags & pair_mask != 0 {
        Some(flags & side_mask != 0)
    } else {
        Some(flags & modifier_mask != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_tokens_are_valid_and_unique() {
        crate::hotkey_listener::tests::assert_keymap_is_consistent(KEYMAP);
    }

    #[test]
    fn escape_maps_to_cancel_token() {
        assert_eq!(token_for_code(53), Some(ESCAPE_TOKEN));
    }

    #[test]
    fn modifier_state_uses_side_specific_bits() {
        let left_shift_held = FLAG_SHIFT | DEVICE_LEFT_SHIFT;
        assert_eq!(modifier_is_down(KVK_SHIFT, left_shift_held), Some(true));
        assert_eq!(
            modifier_is_down(KVK_RIGHT_SHIFT, left_shift_held),
            Some(false)
        );

        // Releasing right Command while left Command stays held.
        let left_command_held = FLAG_COMMAND | DEVICE_LEFT_COMMAND;
        assert_eq!(
            modifier_is_down(KVK_RIGHT_COMMAND, left_command_held),
            Some(false)
        );
        assert_eq!(modifier_is_down(KVK_COMMAND, left_command_held), Some(true));

        assert_eq!(modifier_is_down(KVK_CONTROL, 0), Some(false));
    }

    #[test]
    fn modifier_state_falls_back_without_side_bits() {
        assert_eq!(modifier_is_down(KVK_OPTION, FLAG_ALTERNATE), Some(true));
        assert_eq!(modifier_is_down(KVK_RIGHT_OPTION, 0), Some(false));
    }

    #[test]
    fn function_and_caps_lock_follow_their_flags() {
        assert_eq!(
            modifier_is_down(KVK_FUNCTION, FLAG_SECONDARY_FN),
            Some(true)
        );
        assert_eq!(modifier_is_down(KVK_FUNCTION, 0), Some(false));
        assert_eq!(
            modifier_is_down(KVK_CAPS_LOCK, FLAG_ALPHA_SHIFT),
            Some(true)
        );
        assert_eq!(modifier_is_down(0, FLAG_SHIFT), None);
    }
}
