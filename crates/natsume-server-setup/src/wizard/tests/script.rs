use super::*;
use crate::interaction::{Choice, PromptId, notice::Notice};
use secrecy::ExposeSecret as _;
use std::collections::VecDeque;

pub(super) enum Reply {
    Text(String),
    Date(time::Date),
    Default,
    Secret(SecretString),
    Select(usize),
    Confirm(bool),
    Back,
    Interrupt,
}
impl Reply {
    /// Builds a calendar answer from a fixture's UTC date.
    pub fn date(timestamp: &str) -> Self {
        match time::OffsetDateTime::parse(timestamp, &time::format_description::well_known::Rfc3339)
        {
            Ok(timestamp) => Self::Date(timestamp.date()),
            Err(error) => panic!("invalid scripted date {timestamp:?}: {error}"),
        }
    }
    pub fn text(text: &str) -> Self {
        Self::Text(text.into())
    }
    pub fn secret(text: &str) -> Self {
        Self::Secret(SecretString::from(text))
    }
}

pub(super) struct Selection {
    pub id: PromptId,
    pub choices: Vec<(String, Option<String>)>,
    pub default: usize,
}

pub(super) struct ScriptedInteraction {
    expected: VecDeque<(PromptId, Reply)>,
    pub notices: Vec<Notice>,
    pub summary_error: Option<InteractionError>,
    pub summary_interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub signing_interrupt: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Pending file changed when the final summary is displayed.
    pub summary_changed_file: Option<PathBuf>,
    pub selections: Vec<Selection>,
    pub defaults: Vec<(PromptId, String)>,
    /// Calendar defaults and lower bounds observed by the script.
    pub date_defaults: Vec<(PromptId, time::Date, Option<time::Date>)>,
    pub reveals: usize,
    pub clears: usize,
}

impl ScriptedInteraction {
    pub fn new(prompts: Vec<(PromptId, Reply)>) -> Self {
        Self {
            expected: prompts.into(),
            notices: Vec::new(),
            summary_error: None,
            summary_interrupt: None,
            signing_interrupt: None,
            summary_changed_file: None,
            selections: Vec::new(),
            defaults: Vec::new(),
            date_defaults: Vec::new(),
            reveals: 0,
            clears: 0,
        }
    }
    pub fn assert_finished(&self) {
        assert!(
            self.expected.is_empty(),
            "unconsumed prompt: {:?}",
            self.expected.front().map(|(id, _)| id)
        );
    }
    fn next(&mut self, id: PromptId) -> Result<Reply, InteractionError> {
        let Some((expected, reply)) = self.expected.pop_front() else {
            panic!("unexpected wizard prompt {id:?}")
        };
        assert_eq!(id, expected, "prompt order mismatch");
        if matches!(reply, Reply::Interrupt) {
            Err(InteractionError::Interrupted)
        } else {
            Ok(reply)
        }
    }
}

impl Interaction for ScriptedInteraction {
    fn show(&mut self, notice: &Notice) -> Result<(), InteractionError> {
        self.notices.push(notice.clone());
        if matches!(notice, Notice::OfflineSigningInstructions { .. })
            && let Some(flag) = &self.signing_interrupt
        {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if matches!(notice, Notice::InstallationSummary { .. }) {
            if let Some(flag) = &self.summary_interrupt {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            if let Some(path) = self.summary_changed_file.take() {
                fs::write(path, b"changed pending file")
                    .map_err(|source| InteractionError::Output { source })?;
            }
            if let Some(error) = self.summary_error.take() {
                return Err(error);
            }
        }
        Ok(())
    }
    fn select<T: Clone>(
        &mut self,
        id: PromptId,
        _prompt: &str,
        options: &[Choice<T>],
        default: usize,
    ) -> Result<Answer<T>, InteractionError> {
        self.selections.push(Selection {
            id,
            choices: options
                .iter()
                .map(|option| (option.label.clone(), option.unavailable.clone()))
                .collect(),
            default,
        });
        match self.next(id)? {
            Reply::Back => Ok(Answer::Back),
            Reply::Select(index) => {
                let Some(choice) = options.get(index) else {
                    panic!("script selected a nonexistent option")
                };
                assert!(
                    choice.unavailable.is_none(),
                    "script selected an unavailable option"
                );
                Ok(Answer::Value(choice.value.clone()))
            }
            _ => panic!("expected selection at {id:?}"),
        }
    }
    fn text(
        &mut self,
        id: PromptId,
        _prompt: &str,
        default: Option<&str>,
        _help: Option<&str>,
    ) -> Result<Answer<String>, InteractionError> {
        if let Some(default) = default {
            self.defaults.push((id, default.into()));
        }
        match self.next(id)? {
            Reply::Text(text) => Ok(Answer::Value(text)),
            Reply::Default => {
                let Some(default) = default else {
                    panic!("script requested a missing text default")
                };
                Ok(Answer::Value(default.into()))
            }
            Reply::Back => Ok(Answer::Back),
            _ => panic!("expected text at {id:?}"),
        }
    }
    fn date(
        &mut self,
        id: PromptId,
        _prompt: &str,
        default: time::Date,
        min: Option<time::Date>,
    ) -> Result<Answer<time::Date>, InteractionError> {
        self.date_defaults.push((id, default, min));
        match self.next(id)? {
            Reply::Default => Ok(Answer::Value(default)),
            Reply::Date(date) => {
                assert!(
                    min.is_none_or(|min| date >= min),
                    "script date below calendar minimum"
                );
                Ok(Answer::Value(date))
            }
            Reply::Back => Ok(Answer::Back),
            _ => panic!("expected date at {id:?}"),
        }
    }
    fn confirm(
        &mut self,
        id: PromptId,
        _prompt: &str,
        default: bool,
    ) -> Result<Answer<bool>, InteractionError> {
        assert_eq!(
            default,
            id == PromptId::BootstrapRetry,
            "incorrect confirmation default at {id:?}"
        );
        match self.next(id)? {
            Reply::Confirm(answer) => Ok(Answer::Value(answer)),
            Reply::Back => Ok(Answer::Back),
            _ => panic!("expected confirmation at {id:?}"),
        }
    }
    fn secret(
        &mut self,
        id: PromptId,
        _prompt: &str,
        confirm: bool,
    ) -> Result<Answer<SecretString>, InteractionError> {
        assert_eq!(
            confirm,
            matches!(id, PromptId::NewCaPassphrase { .. }),
            "incorrect secret confirmation at {id:?}"
        );
        match self.next(id)? {
            Reply::Secret(secret) => Ok(Answer::Value(secret)),
            Reply::Back => Ok(Answer::Back),
            _ => panic!("expected secret at {id:?}"),
        }
    }
    fn phrase(
        &mut self,
        id: PromptId,
        _prompt: &str,
        accepted: &[&str],
    ) -> Result<Answer<String>, InteractionError> {
        match self.next(id)? {
            Reply::Text(text) => {
                assert!(
                    accepted.contains(&text.as_str()),
                    "script supplied unaccepted phrase"
                );
                Ok(Answer::Value(text))
            }
            Reply::Back => Ok(Answer::Back),
            _ => panic!("expected phrase at {id:?}"),
        }
    }
    fn reveal_key(&mut self, _role: CaRole, pem: &SecretString) -> Result<(), InteractionError> {
        assert!(
            pem.expose_secret()
                .starts_with("-----BEGIN ENCRYPTED PRIVATE KEY-----"),
            "reveal must contain encrypted PKCS#8 PEM"
        );
        self.reveals += 1;
        Ok(())
    }
    fn clear_scrollback(&mut self) -> Result<(), InteractionError> {
        self.clears += 1;
        Ok(())
    }
}
