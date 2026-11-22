use std::{fs, io, path::Path};

#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub name: String,
    pub background: String,
    pub accent: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub default: String,
    pub schemes: Vec<Scheme>,
}

fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.chars().count() > 40
        || name == "Власна"
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '[' | ']' | '='))
    {
        return Err("Назва має містити 1–40 символів, без дужок і знака =".into());
    }
    Ok(())
}

fn color(value: &str) -> Result<String, String> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Колір має бути у форматі #RRGGBB".into());
    }
    Ok(format!("#{}", value.to_ascii_uppercase()))
}

impl Library {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut result = Self {
            default: String::new(),
            schemes: Vec::new(),
        };
        for (line_number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let name = line[1..line.len() - 1].trim();
                valid_name(name)?;
                if result.schemes.iter().any(|scheme| scheme.name == name) {
                    return Err(format!("Назва схеми повторюється: {name}"));
                }
                result.schemes.push(Scheme {
                    name: name.into(),
                    background: String::new(),
                    accent: String::new(),
                });
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("Некоректний рядок {} у palettes.ini", line_number + 1))?;
            match (result.schemes.last_mut(), key.trim()) {
                (None, "default") => result.default = value.trim().into(),
                (Some(scheme), "background") => scheme.background = color(value)?,
                (Some(scheme), "accent") => scheme.accent = color(value)?,
                _ => {
                    return Err(format!(
                        "Невідоме поле в рядку {} palettes.ini",
                        line_number + 1
                    ));
                }
            }
        }
        if result.schemes.is_empty() {
            return Err("У palettes.ini немає схем".into());
        }
        for scheme in &result.schemes {
            if scheme.background.is_empty() || scheme.accent.is_empty() {
                return Err(format!(
                    "Для схеми «{}» потрібні background та accent",
                    scheme.name
                ));
            }
        }
        if !result
            .schemes
            .iter()
            .any(|scheme| scheme.name == result.default)
        {
            return Err("У palettes.ini вказано невідому основну схему".into());
        }
        Ok(result)
    }

    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let text = include_str!("../assets/palettes.ini");
                fs::write(path, text)?;
                text.into()
            }
            Err(error) => return Err(error),
        };
        Self::parse(&text).map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
    }

    pub fn default_scheme(&self) -> &Scheme {
        self.schemes
            .iter()
            .find(|scheme| scheme.name == self.default)
            .unwrap()
    }

    pub fn index(&self, background: &str, accent: &str) -> i32 {
        self.schemes
            .iter()
            .position(|scheme| scheme.background == background && scheme.accent == accent)
            .map(|index| index as i32)
            .unwrap_or(-1)
    }

    pub fn upsert(&mut self, name: &str, background: &str, accent: &str) -> Result<(), String> {
        let name = name.trim();
        valid_name(name)?;
        let scheme = Scheme {
            name: name.into(),
            background: color(background)?,
            accent: color(accent)?,
        };
        if let Some(existing) = self
            .schemes
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            *existing = scheme;
        } else {
            self.schemes.push(scheme);
        }
        Ok(())
    }

    pub fn encode(&self) -> String {
        let mut text = format!("default={}\n", self.default);
        for scheme in &self.schemes {
            text.push_str(&format!(
                "\n[{}]\nbackground={}\naccent={}\n",
                scheme.name, scheme.background, scheme.accent
            ));
        }
        text
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let temporary = path.with_extension("ini.tmp");
        fs::write(&temporary, self.encode())?;
        fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_config_and_custom_schemes_round_trip_without_losing_mint() {
        let mut library = Library::parse(include_str!("../assets/palettes.ini")).unwrap();
        let mint = library
            .schemes
            .iter()
            .find(|scheme| scheme.name == "М'ята")
            .unwrap()
            .clone();
        library.upsert("Моя схема", "#123456", "#abcdef").unwrap();
        library.upsert("Моя схема", "#234567", "#fedcba").unwrap();
        assert_eq!(library.schemes.len(), 7);
        assert_eq!(library.schemes[6].accent, "#FEDCBA");
        assert!(library.schemes.contains(&mint));
        assert_eq!(Library::parse(&library.encode()).unwrap(), library);
    }

    #[test]
    fn rejects_invalid_configs_and_names_without_changing_the_library() {
        let mut library = Library::parse(include_str!("../assets/palettes.ini")).unwrap();
        let before = library.clone();
        for name in ["", "Власна", "Bad\nname", "[bad]"] {
            assert!(library.upsert(name, "#123456", "#ABCDEF").is_err());
        }
        assert!(library.upsert("Broken", "#xyz", "#ABCDEF").is_err());
        assert_eq!(library, before);
        for config in [
            "",
            "default=Unknown\n[A]\nbackground=#123456\naccent=#ABCDEF",
            "default=A\n[A]\nbackground=#123456",
            "default=A\n[A]\nbackground=#123456\naccent=#ABCDEF\n[A]",
        ] {
            assert!(Library::parse(config).is_err());
        }
    }

    #[test]
    fn custom_schemes_and_manually_changed_default_survive_file_reload() {
        let directory = std::env::temp_dir().join(format!(
            "reword-palettes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("palettes.ini");
        let mut library = Library::load_or_create(&path).unwrap();
        library.upsert("Мій зелений", "#173123", "#AACCAA").unwrap();
        library.save(&path).unwrap();
        library.default = "Мій зелений".into();
        fs::write(&path, library.encode()).unwrap();
        let loaded = Library::load_or_create(&path).unwrap();
        assert_eq!(loaded, library);
        assert_eq!(loaded.default_scheme().name, "Мій зелений");
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&directory).unwrap();
    }
}
