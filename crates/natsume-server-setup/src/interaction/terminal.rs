//! Inquire prompts and explicit controlling-terminal output for one-time key exports.

use super::{
    Answer, CaRole, Choice, Interaction, InteractionError, NoChoicesSnafu, OutputSnafu, PromptId,
    notice::Notice,
};
use inquire::{
    Confirm, DateSelect, Password, PasswordDisplayMode, Select, Text, error::InquireError,
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

fn calendar_date(date: time::Date) -> Result<chrono::NaiveDate, InteractionError> {
    use snafu::OptionExt as _;
    chrono::NaiveDate::from_ymd_opt(
        date.year(),
        u32::from(u8::from(date.month())),
        u32::from(date.day()),
    )
    .context(super::CalendarDateOutOfRangeSnafu)
}

fn selected_date(date: chrono::NaiveDate) -> Result<time::Date, InteractionError> {
    use chrono::Datelike as _;
    let month = u8::try_from(date.month()).context(super::CalendarComponentSnafu)?;
    let month = time::Month::try_from(month).context(super::CalendarDateSnafu)?;
    let day = u8::try_from(date.day()).context(super::CalendarComponentSnafu)?;
    time::Date::from_calendar_date(date.year(), month, day).context(super::CalendarDateSnafu)
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

    fn date(
        &mut self,
        _id: PromptId,
        prompt: &str,
        default: time::Date,
        min: Option<time::Date>,
    ) -> Result<Answer<time::Date>, InteractionError> {
        let mut input = DateSelect::new(prompt)
            .with_default(calendar_date(default)?)
            .with_week_start(chrono::Weekday::Mon);
        if let Some(min) = min {
            input = input.with_min_date(calendar_date(min)?);
        }
        match answer(input.prompt())? {
            Answer::Back => Ok(Answer::Back),
            Answer::Value(date) => Ok(Answer::Value(selected_date(date)?)),
        }
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
    fn calendar_dates_roundtrip_and_out_of_range_dates_retain_the_source()
    -> Result<(), InteractionError> {
        use std::error::Error as _;
        for date in [time::Date::MIN, time::Date::MAX] {
            assert_eq!(selected_date(calendar_date(date)?)?, date);
        }
        let Some(date) = chrono::NaiveDate::from_ymd_opt(2028, 2, 29) else {
            panic!("fixture leap date must exist")
        };
        assert_eq!(calendar_date(selected_date(date)?)?, date);
        let Some(unsupported) = chrono::NaiveDate::from_ymd_opt(10000, 1, 1) else {
            panic!("backend must support fixture year 10000")
        };
        let Err(error) = selected_date(unsupported) else {
            panic!("application must reject year 10000")
        };
        assert!(matches!(error, InteractionError::CalendarDate { .. }));
        assert!(
            error.source().is_some(),
            "calendar conversion must retain its cause"
        );
        Ok(())
    }

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
