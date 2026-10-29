//! Inquire prompts and explicit controlling-terminal output for one-time key exports.

use super::{
    Answer, CaRole, Choice, Interaction, InteractionError, NoChoicesSnafu, OutputSnafu, PromptId,
    notice::Notice,
};
use inquire::{
    Confirm, Password, PasswordDisplayMode, Select, Text, error::InquireError,
    validator::Validation,
};
use secrecy::{ExposeSecret as _, SecretString};
use snafu::{ResultExt as _, ensure};
use std::{fs::OpenOptions, io::Write as _};

/// Interactive terminal implementation without retained input or secret state.
#[derive(Debug, Default)]
pub struct TerminalInteraction;

fn answer<T>(result: Result<T, InquireError>) -> Result<Answer<T>, InteractionError> {
    match result {
        Ok(value) => Ok(Answer::Value(value)),
        Err(InquireError::OperationCanceled) => Ok(Answer::Back),
        Err(InquireError::OperationInterrupted) => Err(InteractionError::Interrupted),
        Err(source) => Err(InteractionError::Terminal { source }),
    }
}

fn write_terminal(text: &str) -> Result<(), InteractionError> {
    let mut terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context(OutputSnafu)?;
    terminal
        .write_all(text.as_bytes())
        .and_then(|()| terminal.flush())
        .context(OutputSnafu)
}

impl Interaction for TerminalInteraction {
    fn show(&mut self, notice: &Notice) -> Result<(), InteractionError> {
        write_terminal(&format!("{notice}\n"))
    }

    fn select<T: Clone>(
        &mut self,
        _id: PromptId,
        prompt: &str,
        options: &[Choice<T>],
        default: usize,
    ) -> Result<Answer<T>, InteractionError> {
        let mut available = Vec::new();
        for (index, choice) in options.iter().enumerate() {
            if let Some(reason) = &choice.unavailable {
                self.show(&Notice::OptionUnavailable {
                    label: choice.label.clone(),
                    reason: reason.clone(),
                })?;
            } else {
                available.push((index, choice));
            }
        }
        ensure!(!available.is_empty(), NoChoicesSnafu);
        let cursor = available
            .iter()
            .position(|(index, _)| *index == default)
            .unwrap_or(0);
        let labels = available
            .iter()
            .map(|(_, choice)| choice.label.as_str())
            .collect();
        match answer(
            Select::new(prompt, labels)
                .with_starting_cursor(cursor)
                .raw_prompt(),
        )? {
            Answer::Back => Ok(Answer::Back),
            Answer::Value(option) => Ok(Answer::Value(available[option.index].1.value.clone())),
        }
    }

    fn text(
        &mut self,
        _id: PromptId,
        prompt: &str,
        default: Option<&str>,
        help: Option<&str>,
    ) -> Result<Answer<String>, InteractionError> {
        let mut input = Text::new(prompt);
        if let Some(default) = default {
            input = input.with_default(default);
        }
        if let Some(help) = help {
            input = input.with_help_message(help);
        }
        answer(input.prompt())
    }

    fn confirm(
        &mut self,
        _id: PromptId,
        prompt: &str,
        default: bool,
    ) -> Result<Answer<bool>, InteractionError> {
        answer(Confirm::new(prompt).with_default(default).prompt())
    }

    fn secret(
        &mut self,
        _id: PromptId,
        prompt: &str,
        confirm: bool,
    ) -> Result<Answer<SecretString>, InteractionError> {
        let mut input = Password::new(prompt)
            .with_display_mode(PasswordDisplayMode::Hidden)
            .with_validator(|text: &str| {
                Ok(if text.is_empty() {
                    Validation::Invalid("Passphrase must not be empty.".into())
                } else {
                    Validation::Valid
                })
            });
        if !confirm {
            input = input.without_confirmation();
        }
        match answer(input.prompt())? {
            Answer::Value(value) => Ok(Answer::Value(SecretString::from(value))),
            Answer::Back => Ok(Answer::Back),
        }
    }

    fn phrase(
        &mut self,
        id: PromptId,
        prompt: &str,
        accepted: &[&str],
    ) -> Result<Answer<String>, InteractionError> {
        ensure!(!accepted.is_empty(), NoChoicesSnafu);
        loop {
            match self.text(id, prompt, None, None)? {
                Answer::Back => return Ok(Answer::Back),
                Answer::Value(value) if accepted.contains(&value.as_str()) => {
                    return Ok(Answer::Value(value));
                }
                Answer::Value(_) => self.show(&Notice::ValidationFailed {
                    message:
                        "Enter one of the accepted phrases exactly, including case and spaces."
                            .into(),
                })?,
            }
        }
    }

    fn reveal_key(&mut self, role: CaRole, pem: &SecretString) -> Result<(), InteractionError> {
        let mut terminal = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context(OutputSnafu)?;
        writeln!(
            terminal,
            "BEGIN ONE-TIME ENCRYPTED {role} PRIVATE KEY EXPORT"
        )
        .and_then(|()| terminal.write_all(pem.expose_secret().as_bytes()))
        .and_then(|()| {
            write!(
                terminal,
                "\nEND ONE-TIME ENCRYPTED {role} PRIVATE KEY EXPORT"
            )
        })
        .and_then(|()| writeln!(terminal, " — save securely before continuing"))
        .and_then(|()| terminal.flush())
        .context(OutputSnafu)
    }

    fn clear_scrollback(&mut self) -> Result<(), InteractionError> {
        write_terminal("\x1b[3J\x1b[H\x1b[2J")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_and_interrupt_have_distinct_semantics() -> Result<(), InteractionError> {
        assert!(matches!(
            answer::<String>(Err(InquireError::OperationCanceled))?,
            Answer::Back
        ));
        assert!(matches!(
            answer::<String>(Err(InquireError::OperationInterrupted)),
            Err(InteractionError::Interrupted)
        ));
        assert!(matches!(
            answer::<String>(Err(InquireError::NotTTY)),
            Err(InteractionError::Terminal { .. })
        ));
        assert!(matches!(answer(Ok(42))?, Answer::Value(42)));
        Ok(())
    }
}
