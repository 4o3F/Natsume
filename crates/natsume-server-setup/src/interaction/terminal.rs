//! Inquire prompts and explicit controlling-terminal output for one-time key exports.

use super::{
    Answer, CaRole, Choice, Interaction, InteractionError, NoChoicesSnafu, OutputSnafu, PromptId,
    notice::{Notice, Tone},
};
use inquire::{
    Confirm, DateSelect, Password, PasswordDisplayMode, Select, Text, error::InquireError,
    validator::Validation,
};
use secrecy::{ExposeSecret as _, SecretString};
use snafu::{ResultExt as _, ensure};
use std::{ffi::OsString, fs::OpenOptions, io::Write as _};

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

const RESET: &str = "\x1b[0m";
const BOLD_YELLOW: &str = "\x1b[1;33m";

/// Marker and SGR sequences that distinguish one notice tone from prompts and other notices.
struct ToneStyle {
    /// Leading symbol kept even without color so severity survives `NO_COLOR`.
    marker: char,
    /// Style of the leading symbol.
    marker_sgr: &'static str,
    /// Style of the notice text, or `None` for the terminal's default color.
    body_sgr: Option<&'static str>,
}

fn tone_style(tone: Tone) -> ToneStyle {
    let (marker, marker_sgr, body_sgr) = match tone {
        Tone::Muted => ('·', "\x1b[2m", Some("\x1b[2m")),
        Tone::Info => ('›', "\x1b[1;36m", None),
        Tone::Attention => ('!', BOLD_YELLOW, Some("\x1b[33m")),
        Tone::Error => ('✗', "\x1b[1;31m", Some("\x1b[31m")),
        Tone::Success => ('✓', "\x1b[1;32m", Some("\x1b[32m")),
    };
    ToneStyle {
        marker,
        marker_sgr,
        body_sgr,
    }
}

/// Follows the `NO_COLOR` convention and skips SGR on terminals that cannot interpret it.
fn color_enabled(no_color: Option<OsString>, term: Option<OsString>) -> bool {
    no_color.is_none_or(|value| value.is_empty())
        && term.is_some_and(|term| !term.is_empty() && term != "dumb")
}

fn terminal_color() -> bool {
    color_enabled(std::env::var_os("NO_COLOR"), std::env::var_os("TERM"))
}

/// Wraps one line in an SGR sequence, resetting before the newline so styles never bleed.
fn paint(text: &str, sgr: Option<&str>, color: bool) -> String {
    match sgr {
        Some(sgr) if color && !text.is_empty() => format!("{sgr}{text}{RESET}"),
        _ => text.to_owned(),
    }
}

/// Renders a notice as a block separated from prompts, led by its tone marker.
fn render(notice: &Notice, color: bool) -> String {
    let style = tone_style(notice.tone());
    let text = notice.to_string();
    let mut rendered = String::from("\n");
    for (index, line) in text.trim_end_matches('\n').lines().enumerate() {
        if index == 0 {
            rendered.push_str(&paint(
                &format!("{} ", style.marker),
                Some(style.marker_sgr),
                color,
            ));
        } else {
            rendered.push_str("  ");
        }
        rendered.push_str(&paint(line, style.body_sgr, color));
        rendered.push('\n');
    }
    rendered
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
        write_terminal(&render(notice, terminal_color()))
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
        // Only the marker lines are styled; the PEM body stays byte-exact for copying.
        let color = terminal_color();
        let begin = paint(
            &format!("BEGIN ONE-TIME ENCRYPTED {role} PRIVATE KEY EXPORT"),
            Some(BOLD_YELLOW),
            color,
        );
        let end = paint(
            &format!(
                "END ONE-TIME ENCRYPTED {role} PRIVATE KEY EXPORT — save securely before continuing"
            ),
            Some(BOLD_YELLOW),
            color,
        );
        writeln!(terminal, "{begin}")
            .and_then(|()| terminal.write_all(pem.expose_secret().as_bytes()))
            .and_then(|()| writeln!(terminal, "\n{end}"))
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
    fn notices_render_as_marked_blocks_without_color() {
        let notice = Notice::ExistingDeployment {
            paths: vec!["/etc/natsume-server/config.toml".into()],
        };
        assert_eq!(
            render(&notice, false),
            "\n✗ This tool supports first installation only. Follow the operations manual for an existing deployment.\n    /etc/natsume-server/config.toml\n"
        );
    }

    #[test]
    fn colored_notices_reset_style_on_every_line() {
        let notice = Notice::PreflightWarnings {
            warnings: vec![
                crate::preflight::PreflightWarning::ClockNotSynchronized,
                crate::preflight::PreflightWarning::TimezoneUnknown,
            ],
        };
        let rendered = render(&notice, true);
        let lines: Vec<_> = rendered.lines().skip(1).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("\x1b[1;33m! \x1b[0m\x1b[33mWarning: "));
        assert!(lines[1].starts_with("  \x1b[33mWarning: "));
        assert!(lines.iter().all(|line| line.ends_with(RESET)));
    }

    #[test]
    fn color_follows_no_color_and_terminal_capability() {
        let set = |value: &str| Some(OsString::from(value));
        assert!(color_enabled(None, set("xterm-256color")));
        assert!(color_enabled(set(""), set("xterm")));
        assert!(!color_enabled(set("1"), set("xterm")));
        assert!(!color_enabled(None, set("dumb")));
        assert!(!color_enabled(None, None));
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
