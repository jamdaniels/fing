use once_cell::sync::Lazy;
use std::sync::RwLock;

pub const DEFAULT_HOTKEY: &str = "F9";

/// Parsed hotkey configuration with modifiers and optional base key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyConfig {
    pub key: Option<HotkeyKey>,
    pub require_ctrl: bool,
    pub require_alt: bool,
    pub require_shift: bool,
    pub require_meta: bool,
    pub require_fn: bool,
}

/// The base key in a hotkey combination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyKey {
    Function,
    F(u8),
    Space,
    Char(char),
}

static HOTKEY_CONFIG: Lazy<RwLock<Option<HotkeyConfig>>> = Lazy::new(|| RwLock::new(None));

const MAX_HOTKEY_LENGTH: usize = 50;
const MAX_HOTKEY_PARTS: usize = 2;
const MAX_HOTKEY_PART_LENGTH: usize = 10;
const INVALID_HOTKEY_MESSAGE: &str =
    "Hotkey must be a single non-space key, a pair of modifiers, or one modifier plus Space";

fn set_base_key(
    current: &mut Option<HotkeyKey>,
    new_key: HotkeyKey,
    full: &str,
) -> Result<(), String> {
    if current.is_some() {
        return Err(format!("Multiple base keys in hotkey: {full}"));
    }
    *current = Some(new_key);
    Ok(())
}

fn parse_function_key(token: &str) -> Option<u8> {
    if !token.starts_with('f') {
        return None;
    }
    let digits = &token[1..];
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let value: u8 = digits.parse().ok()?;
    if (1..=24).contains(&value) {
        Some(value)
    } else {
        None
    }
}

/// Parse a hotkey string like "Ctrl+Shift+F8" into a config.
pub fn parse_hotkey_string(raw: &str) -> Result<HotkeyConfig, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }

    if value.len() > MAX_HOTKEY_LENGTH {
        return Err(format!("Hotkey too long (max {MAX_HOTKEY_LENGTH} chars)"));
    }

    if !value
        .chars()
        .all(|c| c.is_alphanumeric() || c == '+' || c == ' ')
    {
        return Err("Hotkey contains invalid characters".to_string());
    }

    let parts: Vec<&str> = value.split('+').collect();
    if parts.len() > MAX_HOTKEY_PARTS {
        return Err(format!(
            "Too many keys in combination (max {MAX_HOTKEY_PARTS})"
        ));
    }

    let mut require_ctrl = false;
    let mut require_alt = false;
    let mut require_shift = false;
    let mut require_meta = false;
    let mut saw_fn = false;
    let mut base_key: Option<HotkeyKey> = None;

    for part in parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err("Hotkey contains empty key component".to_string());
        }
        if trimmed.len() > MAX_HOTKEY_PART_LENGTH {
            return Err(format!("Invalid key name: {trimmed}"));
        }

        let lower = trimmed.to_lowercase();

        match lower.as_str() {
            "ctrl" | "control" => {
                require_ctrl = true;
                continue;
            }
            "alt" | "option" => {
                require_alt = true;
                continue;
            }
            "shift" => {
                require_shift = true;
                continue;
            }
            "cmd" | "command" | "meta" => {
                require_meta = true;
                continue;
            }
            "fn" => {
                saw_fn = true;
                continue;
            }
            "space" => {
                set_base_key(&mut base_key, HotkeyKey::Space, value)?;
                continue;
            }
            _ => {}
        }

        if trimmed == " " {
            set_base_key(&mut base_key, HotkeyKey::Space, value)?;
            continue;
        }

        if let Some(n) = parse_function_key(&lower) {
            set_base_key(&mut base_key, HotkeyKey::F(n), value)?;
            continue;
        }

        if trimmed.len() == 1 {
            let ch = trimmed.chars().next().unwrap();
            if ch.is_ascii_alphanumeric() {
                set_base_key(
                    &mut base_key,
                    HotkeyKey::Char(ch.to_ascii_uppercase()),
                    value,
                )?;
                continue;
            }
        }

        return Err(format!("Unknown key: {trimmed}"));
    }

    let modifier_count = usize::from(require_ctrl)
        + usize::from(require_alt)
        + usize::from(require_shift)
        + usize::from(require_meta);

    let key = if saw_fn {
        if modifier_count == 0 && base_key.is_none() {
            Some(HotkeyKey::Function)
        } else {
            return Err(INVALID_HOTKEY_MESSAGE.to_string());
        }
    } else {
        match base_key {
            Some(HotkeyKey::Space) => {
                if modifier_count == 1 {
                    Some(HotkeyKey::Space)
                } else {
                    return Err(INVALID_HOTKEY_MESSAGE.to_string());
                }
            }
            Some(key) => {
                if modifier_count == 0 {
                    Some(key)
                } else {
                    return Err(INVALID_HOTKEY_MESSAGE.to_string());
                }
            }
            None => {
                if modifier_count == 2 {
                    None
                } else {
                    return Err(INVALID_HOTKEY_MESSAGE.to_string());
                }
            }
        }
    };

    let require_fn = saw_fn;

    Ok(HotkeyConfig {
        key,
        require_ctrl,
        require_alt,
        require_shift,
        require_meta,
        require_fn,
    })
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

pub fn set_hotkey_from_string_or_default(raw: &str) -> Result<String, String> {
    match set_hotkey_from_string(raw) {
        Ok(()) => Ok(raw.trim().to_string()),
        Err(error) => {
            set_hotkey_from_string(DEFAULT_HOTKEY)?;
            Err(error)
        }
    }
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
    fn accepts_single_non_space_keys() {
        let f9 = parse_hotkey_string("F9").unwrap();
        assert_eq!(f9.key, Some(HotkeyKey::F(9)));

        let a = parse_hotkey_string("A").unwrap();
        assert_eq!(a.key, Some(HotkeyKey::Char('A')));
    }

    #[test]
    fn accepts_modifier_pairs() {
        let config = parse_hotkey_string("Ctrl+Option").unwrap();
        assert_eq!(config.key, None);
        assert!(config.require_ctrl);
        assert!(config.require_alt);
    }

    #[test]
    fn accepts_modifier_plus_space() {
        let config = parse_hotkey_string("Cmd+Space").unwrap();
        assert_eq!(config.key, Some(HotkeyKey::Space));
        assert!(config.require_meta);
    }

    #[test]
    fn rejects_space_only() {
        let error = parse_hotkey_string("Space").unwrap_err();
        assert_eq!(error, INVALID_HOTKEY_MESSAGE);
    }

    #[test]
    fn rejects_modifier_plus_non_space_key() {
        let error = parse_hotkey_string("Ctrl+F9").unwrap_err();
        assert_eq!(error, INVALID_HOTKEY_MESSAGE);
    }

    #[test]
    fn stores_modifier_pair_without_phantom_key() {
        set_hotkey_from_string("Ctrl+Option").unwrap();
        let config = get_hotkey_config().unwrap();
        assert_eq!(config.key, None);
        assert!(config.require_ctrl);
        assert!(config.require_alt);
        assert!(!config.require_shift);
        assert!(!config.require_meta);
        assert!(!config.require_fn);
    }
}
