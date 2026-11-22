#[derive(Default)]
pub struct LiveOutput {
    written: String,
    previous: String,
    blocked: Option<String>,
}

impl LiveOutput {
    pub fn update(
        &mut self,
        text: &str,
        committed: &str,
        finished: bool,
        mut insert: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        if let Some(error) = self.blocked.clone() {
            self.previous = text.to_owned();
            return Err(error);
        }

        let candidate = if finished {
            let text = text.trim();
            if text.is_empty() {
                String::new()
            } else {
                format!("{text} ")
            }
        } else {
            let stable = common_prefix_at_whitespace(&self.previous, text);
            let committed = committed.trim_end();
            let committed = if committed.is_empty() {
                String::new()
            } else {
                format!("{committed} ")
            };
            if committed.chars().count() > stable.chars().count() {
                committed
            } else {
                stable
            }
        };
        self.previous = text.to_owned();

        if candidate.starts_with(self.written.as_str()) {
            let suffix = &candidate[self.written.len()..];
            if !suffix.is_empty() {
                if let Err(error) = insert(suffix) {
                    self.blocked = Some(error.clone());
                    return Err(error);
                }
                self.written.push_str(suffix);
            }
            return Ok(());
        }

        if !finished && self.written.starts_with(&candidate) {
            return Ok(());
        }

        let error =
            "Текст було уточнено після введення; повний результат доступний у Налаштуваннях.";
        self.blocked = Some(error.to_owned());
        Err(error.to_owned())
    }

    pub fn written(&self) -> &str {
        &self.written
    }
}

fn common_prefix_at_whitespace(left: &str, right: &str) -> String {
    let mut prefix = String::new();
    for (left, right) in left.chars().zip(right.chars()) {
        if left != right {
            break;
        }
        prefix.push(left);
    }

    if let Some((index, whitespace)) = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
    {
        prefix.truncate(index + whitespace.len_utf8());
    } else {
        prefix.clear();
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ukrainian_multibyte_prefix_is_confirmed_at_a_word_boundary() {
        let mut output = LiveOutput::default();
        let mut inserted = String::new();

        output
            .update("Привіт світе", "", false, |suffix| {
                inserted.push_str(suffix);
                Ok(())
            })
            .unwrap();
        assert!(inserted.is_empty());

        output
            .update("Привіт світанку", "", false, |suffix| {
                inserted.push_str(suffix);
                Ok(())
            })
            .unwrap();
        assert_eq!(inserted, "Привіт ");
        assert_eq!(output.written(), "Привіт ");
    }

    #[test]
    fn unchanged_hypothesis_does_not_insert_twice() {
        let mut output = LiveOutput::default();
        let mut inserted = String::new();
        for _ in 0..3 {
            output
                .update("Добрий день", "", false, |suffix| {
                    inserted.push_str(suffix);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(inserted, "Добрий ");
    }

    #[test]
    fn committed_phrase_is_inserted_immediately() {
        let mut output = LiveOutput::default();
        let mut inserted = String::new();

        output
            .update("Добрий день", "Добрий", false, |suffix| {
                inserted.push_str(suffix);
                Ok(())
            })
            .unwrap();

        assert_eq!(inserted, "Добрий ");
        assert_eq!(output.written(), "Добрий ");
    }

    #[test]
    fn final_suffix_and_space_are_appended_once() {
        let mut output = LiveOutput::default();
        let mut inserted = String::new();
        output
            .update("Добрий день", "Добрий", false, |suffix| {
                inserted.push_str(suffix);
                Ok(())
            })
            .unwrap();

        for _ in 0..2 {
            output
                .update(
                    "Добрий день",
                    "Добрий день",
                    true,
                    |suffix| {
                        inserted.push_str(suffix);
                        Ok(())
                    },
                )
                .unwrap();
        }

        assert_eq!(inserted, "Добрий день ");
        assert_eq!(output.written(), "Добрий день ");
    }

    #[test]
    fn insertion_failure_is_latched_and_never_retried() {
        let mut output = LiveOutput::default();
        let mut calls = 0;
        let first = output
            .update(
                "Перевірка тексту",
                "Перевірка",
                false,
                |_| {
                    calls += 1;
                    Err("focus lost".to_owned())
                },
            )
            .unwrap_err();
        assert_eq!(first, "focus lost");

        let later = output
            .update(
                "Перевірка тексту",
                "Перевірка",
                false,
                |_| {
                    calls += 1;
                    Ok(())
                },
            )
            .unwrap_err();
        assert_eq!(later, first);
        assert_eq!(calls, 1);
        assert!(output.written().is_empty());
    }

    #[test]
    fn revised_prefix_does_not_rewrite_inserted_content() {
        let mut output = LiveOutput::default();
        let mut inserted = String::new();
        output
            .update("Привіт світе", "Привіт", false, |suffix| {
                inserted.push_str(suffix);
                Ok(())
            })
            .unwrap();
        let before = inserted.clone();

        let error = output
            .update("Привіт, друзі", "Привіт", true, |suffix| {
                inserted.push_str(suffix);
                Ok(())
            })
            .unwrap_err();

        assert!(error.contains("уточнено"));
        assert!(error.contains("Налаштуваннях"));
        assert_eq!(inserted, before);
        assert_eq!(output.written(), before);
    }
}
