use once_cell::sync::Lazy;
use std::collections::HashSet;
use std::sync::RwLock;

/// Parsed hotkey configuration as an ordered set of physical key tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub key_set: HashSet<String>,
    pub keys: Vec<String>,
}

static HOTKEY_CONFIG: Lazy<RwLock<Option<HotkeyConfig>>> = Lazy::new(|| RwLock::new(None));

const MAX_HOTKEY_LENGTH: usize = 200;
const MAX_HOTKEY_PARTS: usize = 16;

fn is_function_key(token: &str) -> bool {
    let Some(digits) = token.strip_prefix('F') else {
        return false;
    };
    if digits.is_empty() || digits.len() > 2 {
        return false;
    }
    let Ok(value) = digits.parse::<u8>() else {
        return false;
    };
    (1..=24).contains(&value)
}

fn is_letter_key(token: &str) -> bool {
    let Some(letter) = token.strip_prefix("Key") else {
        return false;
    };
    letter.len() == 1 && letter.as_bytes()[0].is_ascii_uppercase()
}

fn is_number_key(token: &str, prefix: &str) -> bool {
    let Some(number) = token.strip_prefix(prefix) else {
        return false;
    };
    number.len() == 1 && number.as_bytes()[0].is_ascii_digit()
}

fn is_valid_hotkey_token(token: &str) -> bool {
    if is_function_key(token)
        || is_letter_key(token)
        || is_number_key(token, "Num")
        || is_number_key(token, "Kp")
    {
        return true;
    }

    matches!(
        token,
        "Alt"
            | "AltGr"
            | "Apps"
            | "Backslash"
            | "Backspace"
            | "BackQuote"
            | "Cancel"
            | "CapsLock"
            | "Clear"
            | "Comma"
            | "ControlLeft"
            | "ControlRight"
            | "Delete"
            | "Dot"
            | "DownArrow"
            | "End"
            | "Equal"
            | "Execute"
            | "Final"
            | "Function"
            | "Hangul"
            | "Hanja"
            | "Hanji"
            | "Help"
            | "Home"
            | "Insert"
            | "IntlBackslash"
            | "IntlRo"
            | "IntlYen"
            | "Junja"
            | "Kana"
            | "KanaMode"
            | "KpComma"
            | "KpDecimal"
            | "KpDivide"
            | "KpEqual"
            | "KpMinus"
            | "KpMultiply"
            | "KpPlus"
            | "KpReturn"
            | "Lang1"
            | "Lang2"
            | "Lang3"
            | "Lang4"
            | "Lang5"
            | "LeftArrow"
            | "LeftBracket"
            | "MetaLeft"
            | "MetaRight"
            | "Minus"
            | "NumLock"
            | "PageDown"
            | "PageUp"
            | "Pause"
            | "Print"
            | "PrintScreen"
            | "Quote"
            | "Return"
            | "RightArrow"
            | "RightBracket"
            | "ScrollLock"
            | "Select"
            | "Separator"
            | "SemiColon"
            | "ShiftLeft"
            | "ShiftRight"
            | "Slash"
            | "Sleep"
            | "Space"
            | "Tab"
            | "UpArrow"
            | "VolumeDown"
            | "VolumeMute"
            | "VolumeUp"
    )
}

/// Parse a canonical hotkey string like "ControlLeft+KeyK" into a config.
pub fn parse_hotkey_string(raw: &str) -> Result<HotkeyConfig, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }

    if value.len() > MAX_HOTKEY_LENGTH {
        return Err(format!("Hotkey too long (max {MAX_HOTKEY_LENGTH} chars)"));
    }

    let parts: Vec<&str> = value.split('+').collect();
    if parts.len() > MAX_HOTKEY_PARTS {
        return Err(format!(
            "Too many keys in combination (max {MAX_HOTKEY_PARTS})"
        ));
    }

    let mut key_set = HashSet::new();
    let mut keys = Vec::new();

    for part in parts {
        let token = part.trim();
        if token.is_empty() {
            return Err("Hotkey contains empty key component".to_string());
        }
        if token == "Escape" {
            return Err("Escape cannot be used as a hotkey".to_string());
        }
        if !is_valid_hotkey_token(token) {
            return Err(format!("Unknown key token: {token}"));
        }
        if !key_set.insert(token.to_string()) {
            return Err(format!("Duplicate key token in hotkey: {token}"));
        }
        keys.push(token.to_string());
    }

    Ok(HotkeyConfig { key_set, keys })
}

/// Parse and set the global hotkey configuration.
pub fn set_hotkey_from_string(raw: &str) -> Result<(), String> {
    let config = parse_hotkey_string(raw)?;

    let mut guard = HOTKEY_CONFIG
        .write()
        .map_err(|e| format!("Hotkey config lock poisoned: {e}"))?;
    *guard = Some(config);

    tracing::info!("Hotkey updated to: {}", raw);
    Ok(())
}

pub fn clear_hotkey_config() -> Result<(), String> {
    let mut guard = HOTKEY_CONFIG
        .write()
        .map_err(|e| format!("Hotkey config lock poisoned: {e}"))?;
    *guard = None;
    Ok(())
}

pub fn get_hotkey_config() -> Option<HotkeyConfig> {
    HOTKEY_CONFIG
        .read()
        .ok()
        .and_then(|cfg| cfg.as_ref().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_single_keys() {
        let f9 = parse_hotkey_string("F9").unwrap();
        assert_eq!(f9.keys, vec!["F9"]);

        let a = parse_hotkey_string("KeyA").unwrap();
        assert_eq!(a.keys, vec!["KeyA"]);

        let shift = parse_hotkey_string("ShiftLeft").unwrap();
        assert_eq!(shift.keys, vec!["ShiftLeft"]);
    }

    #[test]
    fn accepts_arbitrary_key_sets() {
        let config = parse_hotkey_string("ControlLeft+KeyK+Space").unwrap();
        assert_eq!(config.keys, vec!["ControlLeft", "KeyK", "Space"]);
        assert!(config.key_set.contains("ControlLeft"));
        assert!(config.key_set.contains("KeyK"));
        assert!(config.key_set.contains("Space"));
    }

    #[test]
    fn rejects_old_format_hotkeys() {
        assert!(parse_hotkey_string("Ctrl+Option").is_err());
        assert!(parse_hotkey_string("Cmd+Space").is_err());
        assert!(parse_hotkey_string("A").is_err());
        assert!(parse_hotkey_string("Fn").is_err());
    }

    #[test]
    fn rejects_escape() {
        assert_eq!(
            parse_hotkey_string("Escape").unwrap_err(),
            "Escape cannot be used as a hotkey"
        );
        assert_eq!(
            parse_hotkey_string("ControlLeft+Escape").unwrap_err(),
            "Escape cannot be used as a hotkey"
        );
    }

    #[test]
    fn rejects_empty_and_duplicate_tokens() {
        assert!(parse_hotkey_string("").is_err());
        assert!(parse_hotkey_string("ControlLeft+").is_err());
        assert!(parse_hotkey_string("KeyA+KeyA").is_err());
    }

    #[test]
    fn stores_key_set_config() {
        set_hotkey_from_string("ControlLeft+KeyK").unwrap();
        let config = get_hotkey_config().unwrap();
        assert_eq!(config.keys, vec!["ControlLeft", "KeyK"]);
        assert!(config.key_set.contains("ControlLeft"));
        assert!(config.key_set.contains("KeyK"));
    }
}
