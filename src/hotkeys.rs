use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use std::{fs, path::Path};

pub const DEFAULT: &str = "Ctrl+Alt+H";

pub fn parse(input: &str) -> Result<HotKey, String> {
    let normalized = input
        .split('+')
        .map(|part| match part.trim().to_ascii_lowercase().as_str() {
            "win" | "windows" => "Meta".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+");
    let shortcut: HotKey = normalized
        .trim()
        .parse()
        .map_err(|error| format!("Не вдалося розібрати комбінацію: {error}"))?;
    let modifiers = shortcut.mods;
    if modifiers.is_empty()
        && !matches!(
            shortcut.key,
            Code::F1
                | Code::F2
                | Code::F3
                | Code::F4
                | Code::F5
                | Code::F6
                | Code::F7
                | Code::F8
                | Code::F9
                | Code::F10
                | Code::F11
                | Code::F12
                | Code::F13
                | Code::F14
                | Code::F15
                | Code::F16
                | Code::F17
                | Code::F18
                | Code::F19
                | Code::F20
                | Code::F21
                | Code::F22
                | Code::F23
                | Code::F24
        )
    {
        return Err("Додай Ctrl, Alt, Shift або Win; окремо дозволені лише F1–F24".into());
    }
    #[cfg(windows)]
    windows_release_keys(shortcut)?;
    Ok(shortcut)
}

pub fn canonical(shortcut: HotKey) -> String {
    shortcut.to_string()
}

pub fn friendly(shortcut: HotKey) -> String {
    let mut parts = Vec::new();
    if shortcut.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if shortcut.mods.contains(Modifiers::META) {
        #[cfg(target_os = "macos")]
        parts.push("Cmd".to_string());
        #[cfg(windows)]
        parts.push("Win".to_string());
        #[cfg(all(not(windows), not(target_os = "macos")))]
        parts.push("Super".to_string());
    }
    let key = match shortcut.key {
        Code::KeyA => "A",
        Code::KeyB => "B",
        Code::KeyC => "C",
        Code::KeyD => "D",
        Code::KeyE => "E",
        Code::KeyF => "F",
        Code::KeyG => "G",
        Code::KeyH => "H",
        Code::KeyI => "I",
        Code::KeyJ => "J",
        Code::KeyK => "K",
        Code::KeyL => "L",
        Code::KeyM => "M",
        Code::KeyN => "N",
        Code::KeyO => "O",
        Code::KeyP => "P",
        Code::KeyQ => "Q",
        Code::KeyR => "R",
        Code::KeyS => "S",
        Code::KeyT => "T",
        Code::KeyU => "U",
        Code::KeyV => "V",
        Code::KeyW => "W",
        Code::KeyX => "X",
        Code::KeyY => "Y",
        Code::KeyZ => "Z",
        Code::Digit0 => "0",
        Code::Digit1 => "1",
        Code::Digit2 => "2",
        Code::Digit3 => "3",
        Code::Digit4 => "4",
        Code::Digit5 => "5",
        Code::Digit6 => "6",
        Code::Digit7 => "7",
        Code::Digit8 => "8",
        Code::Digit9 => "9",
        Code::Space => "Space",
        Code::F1 => "F1",
        Code::F2 => "F2",
        Code::F3 => "F3",
        Code::F4 => "F4",
        Code::F5 => "F5",
        Code::F6 => "F6",
        Code::F7 => "F7",
        Code::F8 => "F8",
        Code::F9 => "F9",
        Code::F10 => "F10",
        Code::F11 => "F11",
        Code::F12 => "F12",
        Code::F13 => "F13",
        Code::F14 => "F14",
        Code::F15 => "F15",
        Code::F16 => "F16",
        Code::F17 => "F17",
        Code::F18 => "F18",
        Code::F19 => "F19",
        Code::F20 => "F20",
        Code::F21 => "F21",
        Code::F22 => "F22",
        Code::F23 => "F23",
        Code::F24 => "F24",
        _ => return shortcut.to_string().replace('+', " + "),
    };
    parts.push(key.to_string());
    parts.join(" + ")
}

pub fn load(path: &Path) -> Result<HotKey, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return parse(DEFAULT),
        Err(error) => return Err(format!("Не вдалося прочитати hotkeys.ini: {error}")),
    };
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("hotkey="))
        .unwrap_or(DEFAULT);
    parse(value)
}

pub fn save(path: &Path, shortcut: HotKey) -> Result<(), String> {
    let temporary = path.with_extension("ini.tmp");
    fs::write(&temporary, format!("hotkey={}\n", canonical(shortcut)))
        .map_err(|e| format!("Не вдалося записати гарячу клавішу: {e}"))?;
    fs::rename(&temporary, path).map_err(|e| format!("Не вдалося зберегти hotkeys.ini: {e}"))
}

#[cfg(windows)]
pub struct WindowsReleaseKeys {
    pub required: Vec<i32>,
    pub alternatives: Vec<Vec<i32>>,
}

#[cfg(windows)]
pub fn windows_release_keys(shortcut: HotKey) -> Result<WindowsReleaseKeys, String> {
    let mut required = Vec::new();
    let mut alternatives = Vec::new();
    if shortcut.mods.contains(Modifiers::CONTROL) {
        required.push(0x11);
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        required.push(0x12);
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        required.push(0x10);
    }
    if shortcut.mods.contains(Modifiers::META) {
        alternatives.push(vec![0x5B, 0x5C]);
    }
    let key = match shortcut.key {
        Code::KeyA => 0x41,
        Code::KeyB => 0x42,
        Code::KeyC => 0x43,
        Code::KeyD => 0x44,
        Code::KeyE => 0x45,
        Code::KeyF => 0x46,
        Code::KeyG => 0x47,
        Code::KeyH => 0x48,
        Code::KeyI => 0x49,
        Code::KeyJ => 0x4A,
        Code::KeyK => 0x4B,
        Code::KeyL => 0x4C,
        Code::KeyM => 0x4D,
        Code::KeyN => 0x4E,
        Code::KeyO => 0x4F,
        Code::KeyP => 0x50,
        Code::KeyQ => 0x51,
        Code::KeyR => 0x52,
        Code::KeyS => 0x53,
        Code::KeyT => 0x54,
        Code::KeyU => 0x55,
        Code::KeyV => 0x56,
        Code::KeyW => 0x57,
        Code::KeyX => 0x58,
        Code::KeyY => 0x59,
        Code::KeyZ => 0x5A,
        Code::Digit0 => 0x30,
        Code::Digit1 => 0x31,
        Code::Digit2 => 0x32,
        Code::Digit3 => 0x33,
        Code::Digit4 => 0x34,
        Code::Digit5 => 0x35,
        Code::Digit6 => 0x36,
        Code::Digit7 => 0x37,
        Code::Digit8 => 0x38,
        Code::Digit9 => 0x39,
        Code::Space => 0x20,
        Code::F1 => 0x70,
        Code::F2 => 0x71,
        Code::F3 => 0x72,
        Code::F4 => 0x73,
        Code::F5 => 0x74,
        Code::F6 => 0x75,
        Code::F7 => 0x76,
        Code::F8 => 0x77,
        Code::F9 => 0x78,
        Code::F10 => 0x79,
        Code::F11 => 0x7A,
        Code::F12 => 0x7B,
        Code::F13 => 0x7C,
        Code::F14 => 0x7D,
        Code::F15 => 0x7E,
        Code::F16 => 0x7F,
        Code::F17 => 0x80,
        Code::F18 => 0x81,
        Code::F19 => 0x82,
        Code::F20 => 0x83,
        Code::F21 => 0x84,
        Code::F22 => 0x85,
        Code::F23 => 0x86,
        Code::F24 => 0x87,
        _ => {
            return Err(format!(
                "Ця клавіша не підтримує визначення відпускання у Windows: {}",
                shortcut
            ));
        }
    };
    required.push(key);
    Ok(WindowsReleaseKeys {
        required,
        alternatives,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_canonicalizes_aliases_and_allows_function_key_alone() {
        let hotkey = parse("Control + Option + KeyH").unwrap();
        assert_eq!(friendly(hotkey), "Ctrl + Alt + H");
        assert_eq!(parse(&canonical(hotkey)).unwrap(), hotkey);
        assert_eq!(friendly(parse("F9").unwrap()), "F9");
        assert!(parse("H").is_err());
        assert!(parse("Space").is_err());
        assert!(parse("7").is_err());
        assert!(parse("").is_err());
        assert!(parse("Ctrl").is_err());
        assert!(parse("Ctrl++H").is_err());
        assert!(parse("Ctrl+Alt+").is_err());
    }
    #[test]
    fn loads_default_and_custom_configuration() {
        let path = std::env::temp_dir().join(format!("reword-hotkey-{}.ini", std::process::id()));
        let _ = fs::remove_file(&path);
        let default = parse(DEFAULT).unwrap();
        assert_eq!(load(&path).unwrap(), default);
        save(&path, default).unwrap();
        assert_eq!(load(&path).unwrap(), default);
        let custom = parse("Ctrl+Shift+Space").unwrap();
        save(&path, custom).unwrap();
        assert_eq!(load(&path).unwrap(), custom);
        let _ = fs::remove_file(path);
    }
    #[cfg(windows)]
    #[test]
    fn windows_release_mapping_covers_default_space_and_function_keys() {
        assert_eq!(
            windows_release_keys(parse("Ctrl+Alt+H").unwrap())
                .unwrap()
                .required,
            [0x11, 0x12, 0x48]
        );
        assert_eq!(
            windows_release_keys(parse("Ctrl+Shift+Space").unwrap())
                .unwrap()
                .required,
            [0x11, 0x10, 0x20]
        );
        assert_eq!(
            windows_release_keys(parse("F9").unwrap()).unwrap().required,
            [0x78]
        );
        assert_eq!(
            windows_release_keys(parse("Win+H").unwrap())
                .unwrap()
                .alternatives,
            [vec![0x5B, 0x5C]]
        );
    }
}
