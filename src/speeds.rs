use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speeds {
    pub entrance: f32,
    pub exit: f32,
    pub master: f32,
}

impl Default for Speeds {
    fn default() -> Self {
        Self {
            entrance: 1.0,
            exit: 1.0,
            master: 1.0,
        }
    }
}

impl Speeds {
    pub fn new(entrance: f32, exit: f32, master: f32) -> Self {
        let valid = |value: f32| {
            if value.is_finite() {
                value.clamp(0.25, 3.0)
            } else {
                1.0
            }
        };
        Self {
            entrance: valid(entrance),
            exit: valid(exit),
            master: valid(master),
        }
    }

    fn parse(text: &str) -> Self {
        let mut result = Self::default();
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                if let Ok(value) = value.trim().parse::<f32>() {
                    match key.trim() {
                        "entrance" => result.entrance = value,
                        "exit" => result.exit = value,
                        "master" => result.master = value,
                        _ => {}
                    }
                }
            }
        }
        Self::new(result.entrance, result.exit, result.master)
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn effective(self) -> (f32, f32) {
        (self.entrance * self.master, self.exit * self.master)
    }

    fn encode(self) -> String {
        format!(
            "entrance={}\nexit={}\nmaster={}\n",
            self.entrance, self.exit, self.master
        )
    }

    pub fn save(self, path: &Path) -> std::io::Result<()> {
        let temporary = path.with_extension("ini.tmp");
        fs::write(&temporary, self.encode())?;
        fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_multiplier_scales_both_speeds_and_settings_round_trip() {
        let speeds = Speeds::new(2.0, 0.5, 1.5);
        assert_eq!(speeds.effective(), (3.0, 0.75));
        assert_eq!(Speeds::parse(&speeds.encode()), speeds);
    }

    #[test]
    fn malformed_or_missing_settings_cannot_stop_or_reverse_playback() {
        assert_eq!(Speeds::parse(""), Speeds::default());
        assert_eq!(
            Speeds::parse("entrance=NaN\nexit=-2\nmaster=100").effective(),
            (3.0, 0.75)
        );
        assert_eq!(
            Speeds::parse("entrance=garbage\nmaster=inf"),
            Speeds::default()
        );
    }
}
