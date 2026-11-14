use parking_lot::RwLock;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Static exclusion list (`Config/exclusions.txt`) blocking known Star Citizen HUD/NPC words unless listed.
#[derive(Debug, Clone)]
pub struct ExclusionList {
    entries: HashSet<String>,
}

impl ExclusionList {
    pub fn embedded() -> Self {
        Self::parse(include_str!("../../../../assets/exclusions.txt"))
    }

    pub fn load_or_embedded(path: &Path) -> Self {
        if path.exists() {
            if let Ok(text) = fs::read_to_string(path) {
                return Self::parse(&text);
            }
        }
        Self::embedded()
    }

    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut entries = HashSet::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            entries.insert(trimmed.to_ascii_lowercase());
        }
        Self { entries }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains(&name.trim().to_ascii_lowercase())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Dynamic session-level exclusions (e.g., NPC duplicate white nameplates detected by `WhiteDuplicateRule`).
#[derive(Debug, Default)]
pub struct SessionExclusions {
    names: RwLock<HashSet<String>>,
}

impl SessionExclusions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, name: &str) -> bool {
        self.names
            .write()
            .insert(name.trim().to_ascii_lowercase())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names
            .read()
            .contains(&name.trim().to_ascii_lowercase())
    }

    pub fn clear(&self) {
        self.names.write().clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotkeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

impl HotkeyChord {
    pub fn format(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

pub struct HotkeyText;

impl HotkeyText {
    pub fn normalize(raw: &str) -> String {
        if raw.trim().is_empty() {
            return String::new();
        }
        match Self::parse(raw) {
            Ok(c) => c.format(),
            Err(_) => raw.trim().to_string(),
        }
    }

    pub fn parse(raw: &str) -> Result<HotkeyChord, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Hotkey is empty.".to_string());
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut main_key: Option<String> = None;

        for token in trimmed.split('+') {
            let t = token.trim();
            if t.is_empty() {
                return Err(format!("\"{}\" is not a valid hotkey.", raw));
            }
            match t.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "win" | "lwin" | "rwin" | "super" | "meta" => {
                    return Err("Windows/Super key is not supported in hotkeys.".to_string());
                }
                other => {
                    if main_key.is_some() {
                        return Err(format!(
                            "\"{}\" has more than one non-modifier key.",
                            raw
                        ));
                    }
                    main_key = Some(canonical_key_name(other, t)?);
                }
            }
        }

        let Some(key) = main_key else {
            return Err("Hotkey needs a main key alongside modifiers.".to_string());
        };
        Ok(HotkeyChord {
            ctrl,
            alt,
            shift,
            key,
        })
    }
}

fn canonical_key_name(lower: &str, original: &str) -> Result<String, String> {
    if lower.len() == 1 {
        let ch = lower.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            return Ok(ch.to_ascii_uppercase().to_string());
        }
        if ch.is_ascii_digit() {
            return Ok(ch.to_string());
        }
    }
    if let Some(num) = lower.strip_prefix('f') {
        if let Ok(n) = num.parse::<u8>() {
            if (1..=24).contains(&n) {
                return Ok(format!("F{}", n));
            }
        }
    }
    if let Some(num) = lower.strip_prefix('d') {
        if num.len() == 1 && num.chars().all(|c| c.is_ascii_digit()) {
            return Ok(num.to_string());
        }
    }
    match lower {
        "tab" => Ok("Tab".to_string()),
        "space" | "spacebar" => Ok("Space".to_string()),
        "enter" | "return" => Ok("Enter".to_string()),
        "esc" | "escape" => Ok("Escape".to_string()),
        "back" | "backspace" => Ok("Backspace".to_string()),
        "ins" | "insert" => Ok("Insert".to_string()),
        "del" | "delete" => Ok("Delete".to_string()),
        "home" => Ok("Home".to_string()),
        "end" => Ok("End".to_string()),
        "pgup" | "pageup" => Ok("PageUp".to_string()),
        "pgdn" | "pagedown" => Ok("PageDown".to_string()),
        "up" => Ok("Up".to_string()),
        "down" => Ok("Down".to_string()),
        "left" => Ok("Left".to_string()),
        "right" => Ok("Right".to_string()),
        "oemtilde" | "grave" | "backquote" | "`" => Ok("`".to_string()),
        "oemminus" | "minus" | "-" => Ok("-".to_string()),
        "oemplus" | "equals" | "=" => Ok("=".to_string()),
        "oemopenbrackets" | "[" => Ok("[".to_string()),
        "oemclosebrackets" | "]" => Ok("]".to_string()),
        "oempipe" | "backslash" | "\\" => Ok("\\".to_string()),
        "oemsemicolon" | "semicolon" | ";" => Ok(";".to_string()),
        "oemquotes" | "apostrophe" | "'" => Ok("'".to_string()),
        "oemcomma" | "comma" | "," => Ok(",".to_string()),
        "oemperiod" | "period" | "." => Ok(".".to_string()),
        "oemquestion" | "slash" | "/" => Ok("/".to_string()),
        "numpad0" | "num0" => Ok("NumPad0".to_string()),
        "numpad1" | "num1" => Ok("NumPad1".to_string()),
        "numpad2" | "num2" => Ok("NumPad2".to_string()),
        "numpad3" | "num3" => Ok("NumPad3".to_string()),
        "numpad4" | "num4" => Ok("NumPad4".to_string()),
        "numpad5" | "num5" => Ok("NumPad5".to_string()),
        "numpad6" | "num6" => Ok("NumPad6".to_string()),
        "numpad7" | "num7" => Ok("NumPad7".to_string()),
        "numpad8" | "num8" => Ok("NumPad8".to_string()),
        "numpad9" | "num9" => Ok("NumPad9".to_string()),
        _ => Err(format!("Unsupported key \"{}\".", original)),
    }
}
