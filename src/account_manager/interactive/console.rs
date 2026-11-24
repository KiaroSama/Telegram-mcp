use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Write};

pub(super) struct Console {
    color: bool,
    pub quitting: bool,
}

impl Console {
    pub fn new() -> Self {
        Self {
            color: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
            quitting: false,
        }
    }

    fn paint(&self, text: &str, sgr: &str) -> String {
        if self.color {
            format!("\x1b[{sgr}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn menu(&self) {
        println!(
            "\n{}",
            self.paint("Telegram MCP account manager:", "38;5;117")
        );
        for (index, text) in [
            "List configured accounts",
            "Add an account",
            "Remove an account",
            "Rename an account",
            "Log in and save a new account",
            "Check session health / re-login",
        ]
        .into_iter()
        .enumerate()
        {
            println!(
                "  {} {text}",
                self.paint(&format!("{}.", index + 1), "38;5;117")
            );
        }
    }

    fn read(&mut self, prompt: &str) -> Result<Option<String>> {
        print!("{prompt}: ");
        io::stdout().flush()?;
        let mut raw = String::new();
        if io::stdin().read_line(&mut raw)? == 0 || raw.trim().eq_ignore_ascii_case("exit") {
            self.quitting = true;
            return Ok(None);
        }
        Ok(Some(raw.trim().to_owned()))
    }

    pub fn answer(&mut self, prompt: &str, back: bool) -> Result<Option<String>> {
        let quit = self.paint("quit=exit", "38;5;32");
        let hint = if back {
            format!("{{{}, {quit}}}", self.paint("back=0", "38;5;166"))
        } else {
            format!("{{{quit}}}")
        };
        Ok(self
            .read(&format!("{} {hint}", self.paint(prompt, "1")))?
            .filter(|answer| !answer.is_empty() && !(back && answer == "0")))
    }

    pub fn confirm(&mut self, question: &str) -> Result<bool> {
        Ok(self
            .read(&format!(
                "{} {}",
                self.paint(question, "1"),
                self.paint("[Y/n]", "92")
            ))?
            .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "" | "y" | "yes")))
    }

    pub fn number(&mut self, prompt: &str, count: usize) -> Result<Option<usize>> {
        while let Some(answer) = self.answer(prompt, true)? {
            if answer.bytes().all(|byte| byte.is_ascii_digit())
                && let Ok(number) = answer.parse::<usize>()
                && (1..=count).contains(&number)
            {
                return Ok(Some(number - 1));
            }
            println!("Enter a number from 1 to {count}.");
        }
        Ok(None)
    }

    pub fn label(&mut self, prompt: &str) -> Result<Option<String>> {
        while let Some(raw) = self.answer(prompt, true)? {
            if let Some(label) = normalized_label(&raw) {
                if !raw.eq_ignore_ascii_case(&label) {
                    println!("Stored as '{label}'. Use this value as account=.");
                }
                return Ok(Some(label));
            }
            println!(
                "Use up to 64 letters, digits or underscores. Spaces and hyphens become underscores."
            );
        }
        Ok(None)
    }

    pub fn secret(&mut self, prompt: &str) -> Result<String> {
        if io::stdin().is_terminal() {
            let value = rpassword::prompt_password(format!("{prompt}: "))
                .context("Could not read hidden session input")?;
            if value.trim().eq_ignore_ascii_case("exit") {
                self.quitting = true;
                return Ok(String::new());
            }
            return Ok(value);
        }
        // Piped input is already private and must not be echoed back to the caller.
        println!("{prompt}:");
        let mut value = String::new();
        if io::stdin().read_line(&mut value)? == 0 || value.trim().eq_ignore_ascii_case("exit") {
            self.quitting = true;
        }
        Ok(value)
    }
}

fn normalized_label(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_ -".contains(&b))
    {
        return None;
    }
    let label = raw
        .split([' ', '-'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let label = label.trim_matches('_').to_ascii_lowercase();
    crate::session::validate_label(&label).ok()?;
    Some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_legacy_menu_normalization_and_native_size_limits() {
        for (raw, expected) in [
            ("  KGB Verifier  ", Some("kgb_verifier")),
            ("alpha-beta", Some("alpha_beta")),
            ("__a  - b__", Some("a_b")),
            ("alpha__beta", Some("alpha__beta")),
            ("A @ B", None),
            ("   ", None),
            ("کار", None),
            ("____", None),
        ] {
            assert_eq!(normalized_label(raw).as_deref(), expected);
        }
        assert!(normalized_label(&"a".repeat(65)).is_none());
    }

    #[test]
    fn console_colors_leave_prompt_text_readable_without_color() {
        let console = Console {
            color: false,
            quitting: false,
        };
        assert_eq!(console.paint("back=0", "38;5;166"), "back=0");
        let console = Console {
            color: true,
            quitting: false,
        };
        assert_eq!(
            console.paint("quit=exit", "38;5;32"),
            "\x1b[38;5;32mquit=exit\x1b[0m"
        );
    }
}
