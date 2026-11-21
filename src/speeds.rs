use std::{fs, path::Path};

// The user's chosen tempo is the new 1x reference for fresh installations too.
const PREVIOUS_ENTRANCE: f32 = 1.3162501;
const PREVIOUS_EXIT: f32 = 1.385;
const PREVIOUS_MASTER: f32 = 1.59125;
const BASE_ENTRANCE: f32 = PREVIOUS_ENTRANCE * PREVIOUS_MASTER;
const BASE_EXIT: f32 = PREVIOUS_EXIT * PREVIOUS_MASTER;

pub fn parse_multiplier(text: &str) -> Result<f32, &'static str> {
    let value = text
        .trim()
        .trim_end_matches('×')
        .trim()
        .replace(',', ".")
        .parse::<f32>()
        .map_err(|_| "Введи число від 0,25 до 3")?;
    if !value.is_finite() || !(0.25..=3.0).contains(&value) {
        return Err("Введи число від 0,25 до 3");
    }
    Ok(value)
}

pub fn display_multiplier(value: f32) -> String {
    value.to_string()
}

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
        let current_basis = text.lines().any(|line| line.trim() == "basis=2");
        let mut has_values = false;
        for line in text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                if let Ok(value) = value.trim().parse::<f32>() {
                    match key.trim() {
                        "entrance" => {
                            result.entrance = value;
                            has_values = true;
                        }
                        "exit" => {
                            result.exit = value;
                            has_values = true;
                        }
                        "master" => {
                            result.master = value;
                            has_values = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        let result = Self::new(result.entrance, result.exit, result.master);
        if has_values && !current_basis {
            Self::new(
                result.entrance / PREVIOUS_ENTRANCE,
                result.exit / PREVIOUS_EXIT,
                result.master / PREVIOUS_MASTER,
            )
        } else {
            result
        }
    }

    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn effective(self) -> (f32, f32) {
        (
            BASE_ENTRANCE * self.entrance * self.master,
            BASE_EXIT * self.exit * self.master,
        )
    }

    fn encode(self) -> String {
        format!(
            "basis=2\nentrance={}\nexit={}\nmaster={}\n",
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
        let (open, close) = speeds.effective();
        assert!((open - BASE_ENTRANCE * 3.0).abs() < 0.00001);
        assert!((close - BASE_EXIT * 0.75).abs() < 0.00001);
        assert_eq!(Speeds::parse(&speeds.encode()), speeds);
    }

    #[test]
    fn malformed_or_missing_settings_cannot_stop_or_reverse_playback() {
        assert_eq!(Speeds::parse(""), Speeds::default());
        assert_eq!(
            Speeds::parse("basis=2\nentrance=NaN\nexit=-2\nmaster=100"),
            Speeds::new(1.0, 0.25, 3.0)
        );
        assert_eq!(
            Speeds::parse("basis=2\nentrance=garbage\nmaster=inf"),
            Speeds::default()
        );
    }

    #[test]
    fn selected_legacy_settings_become_one_without_changing_effective_tempo() {
        let migrated = Speeds::parse("entrance=1.3162501\nexit=1.385\nmaster=1.59125");
        assert_eq!(migrated, Speeds::default());
        assert_eq!(migrated.effective(), (BASE_ENTRANCE, BASE_EXIT));
        assert_eq!(Speeds::parse(&migrated.encode()), migrated);
    }

    #[test]
    fn manual_input_accepts_decimal_comma_and_rejects_invalid_values() {
        assert_eq!(parse_multiplier("1,25"), Ok(1.25));
        assert_eq!(parse_multiplier(" 2.5× "), Ok(2.5));
        assert_eq!(parse_multiplier("0.25"), Ok(0.25));
        for text in ["", "-1", "0", "3.1", "NaN", "inf", "hello"] {
            assert!(parse_multiplier(text).is_err());
        }
        assert_eq!(display_multiplier(1.0), "1");
        assert_eq!(display_multiplier(1.25), "1.25");
        assert_eq!(parse_multiplier(&display_multiplier(1.23456)), Ok(1.23456));
    }
}
