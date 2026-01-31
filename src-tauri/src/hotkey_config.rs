use once_cell::sync::Lazy;
use std::sync::RwLock;

#[derive(Clone, Debug)]
pub struct HotkeyConfig {
    pub key: HotkeyKey,
    pub require_ctrl: bool,
    pub require_alt: bool,
    pub require_shift: bool,
    pub require_meta: bool,
    pub require_fn: bool,
}

#[derive(Clone, Debug)]
pub enum HotkeyKey {
    Function,
    F(u8),
    Space,
    Char(char),
}

static HOTKEY_CONFIG: Lazy<RwLock<Option<HotkeyConfig>>> = Lazy::new(|| RwLock::new(None));

const MAX_HOTKEY_LENGTH: usize = 50;
const MAX_HOTKEY_PARTS: usize = 5;
const MAX_HOTKEY_PART_LENGTH: usize = 10;

fn set_base_key(
    current: &mut Option<HotkeyKey>,
    new_key: HotkeyKey,
    full: &str,
) -> Result<(), String> {
    if current.is_some() {
        return Err(format!("Multiple base keys in hotkey: {}", full));
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

pub fn parse_hotkey_string(raw: &str) -> Result<HotkeyConfig, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }

    if value.len() > MAX_HOTKEY_LENGTH {
        return Err(format!("Hotkey too long (max {} chars)", MAX_HOTKEY_LENGTH));
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
            "Too many keys in combination (max {})",
            MAX_HOTKEY_PARTS
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
            return Err(format!("Invalid key name: {}", trimmed));
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

        return Err(format!("Unknown key: {}", trimmed));
    }

    let require_fn = saw_fn;

    let key = match base_key {
        Some(k) => k,
        None => {
            if require_fn && !require_ctrl && !require_alt && !require_shift && !require_meta {
                HotkeyKey::Function
            } else {
                return Err("Hotkey must include a base key".to_string());
            }
        }
    };

    Ok(HotkeyConfig {
        key,
        require_ctrl,
        require_alt,
        require_shift,
        require_meta,
        require_fn,
    })
}

pub fn set_hotkey_from_string(raw: &str) -> Result<(), String> {
    let config = parse_hotkey_string(raw)?;

    let mut guard = HOTKEY_CONFIG
        .write()
        .map_err(|e| format!("Hotkey config lock poisoned: {}", e))?;
    *guard = Some(config);

    tracing::info!("Hotkey updated to: {}", raw);
    Ok(())
}

pub fn get_hotkey_config() -> Option<HotkeyConfig> {
    HOTKEY_CONFIG
        .read()
        .ok()
        .and_then(|cfg| cfg.as_ref().cloned())
}

/// Get the current hotkey as a string for frontend use
pub fn get_hotkey_string() -> String {
    let config = match get_hotkey_config() {
        Some(c) => c,
        None => return "F8".to_string(), // Default
    };

    let mut parts = Vec::new();

    if config.require_ctrl {
        parts.push("Ctrl");
    }
    if config.require_alt {
        parts.push("Alt");
    }
    if config.require_shift {
        parts.push("Shift");
    }
    if config.require_meta {
        parts.push("Meta");
    }
    if config.require_fn {
        parts.push("Fn");
    }

    let key_str = match config.key {
        HotkeyKey::Function => "Fn".to_string(),
        HotkeyKey::F(n) => format!("F{}", n),
        HotkeyKey::Space => "Space".to_string(),
        HotkeyKey::Char(c) => c.to_string(),
    };
    parts.push(&key_str);

    parts.join("+")
}
