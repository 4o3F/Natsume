use super::{Host, Interaction, Step, Wizard, WizardFailure, value};
use crate::{
    config::{ConfigRenderError, DeploymentParameters, ParameterRejection},
    interaction::{
        PromptId,
        notice::{self, Notice},
    },
};
use std::net::IpAddr;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Ips,
    Port,
    Hostname,
    Origin,
    ContestEnd,
    GatewayExpiry,
}

impl Field {
    pub(super) const fn prompt(self) -> PromptId {
        match self {
            Self::Ips => PromptId::ServerIps,
            Self::Port => PromptId::HttpsPort,
            Self::Hostname => PromptId::GatewayHostname,
            Self::Origin => PromptId::DomjudgeOrigin,
            Self::ContestEnd => PromptId::ContestEnd,
            Self::GatewayExpiry => PromptId::GatewayNotAfter,
        }
    }
    pub(super) const fn previous(self) -> Option<Self> {
        match self {
            Self::Ips => None,
            Self::Port => Some(Self::Ips),
            Self::Hostname => Some(Self::Port),
            Self::Origin => Some(Self::Hostname),
            Self::ContestEnd => Some(Self::Origin),
            Self::GatewayExpiry => Some(Self::ContestEnd),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Draft {
    ips: Option<Vec<IpAddr>>,
    port: Option<u16>,
    hostname: Option<String>,
    origin: Option<String>,
    contest_end: Option<String>,
    gateway_expiry: Option<String>,
}

pub(super) const fn rejection_prompt(reason: ParameterRejection) -> PromptId {
    match reason {
        ParameterRejection::EmptyServerIps
        | ParameterRejection::TooManyServerIps
        | ParameterRejection::UnusableServerIp => PromptId::ServerIps,
        ParameterRejection::HttpsPortOutOfRange => PromptId::HttpsPort,
        ParameterRejection::EmptyGatewayHostname => PromptId::GatewayHostname,
        ParameterRejection::EmptyDomjudgeOrigin => PromptId::DomjudgeOrigin,
        ParameterRejection::ContestEndFormat => PromptId::ContestEnd,
        ParameterRejection::GatewayNotAfterFormat
        | ParameterRejection::GatewayValidityTooShort
        | ParameterRejection::GatewayExpired => PromptId::GatewayNotAfter,
    }
}

fn rejection_field(error: &ConfigRenderError) -> Option<Field> {
    let reason = match error {
        ConfigRenderError::InvalidParameter { reason }
        | ConfigRenderError::TimestampParse { reason, .. } => *reason,
        _ => return None,
    };
    Some(match rejection_prompt(reason) {
        PromptId::ServerIps => Field::Ips,
        PromptId::HttpsPort => Field::Port,
        PromptId::GatewayHostname => Field::Hostname,
        PromptId::DomjudgeOrigin => Field::Origin,
        PromptId::ContestEnd => Field::ContestEnd,
        _ => Field::GatewayExpiry,
    })
}

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn parameter(&mut self, field: Field) -> Result<Step, WizardFailure> {
        let default = match field {
            Field::Port => Some(self.answers.draft.port.unwrap_or(8443).to_string()),
            Field::GatewayExpiry => self.answers.draft.gateway_expiry.clone().or_else(|| {
                let end =
                    OffsetDateTime::parse(self.answers.draft.contest_end.as_deref()?, &Rfc3339)
                        .ok()?;
                end.checked_add(Duration::days(1))?.format(&Rfc3339).ok()
            }),
            _ => None,
        };
        let id = field.prompt();
        let text =
            value(
                self.interaction
                    .text(id, notice::prompt(id), default.as_deref(), None)?,
            )?;
        let valid = self.answers.draft.set(field, text);
        if !valid {
            let message = match field {
                Field::Ips => notice::INVALID_IP,
                Field::Port => notice::INVALID_PORT,
                _ => notice::INVALID_TIME,
            };
            self.rejected(message)?;
            return Ok(Step::Parameters(field));
        }
        let draft = &self.answers.draft;
        if let (Some(ips), Some(port), Some(hostname), Some(origin), Some(end), Some(expiry)) = (
            &draft.ips,
            draft.port,
            &draft.hostname,
            &draft.origin,
            &draft.contest_end,
            &draft.gateway_expiry,
        ) {
            match DeploymentParameters::new(
                ips.clone(),
                port,
                hostname.clone(),
                origin.clone(),
                end.clone(),
                expiry.clone(),
                self.host.now(),
            ) {
                Ok(params) => {
                    self.answers.params = Some(params);
                    return Ok(Step::ControlCa);
                }
                Err(error) => {
                    if let Some(retry) = rejection_field(&error) {
                        self.rejected(&snafu::Report::from_error(&error).to_string())?;
                        return Ok(Step::Parameters(retry));
                    }
                    return Err(error.into());
                }
            }
        }
        let next = match field {
            Field::Ips => Field::Port,
            Field::Port => Field::Hostname,
            Field::Hostname => Field::Origin,
            Field::Origin => Field::ContestEnd,
            Field::ContestEnd => Field::GatewayExpiry,
            Field::GatewayExpiry => unreachable!("all parameter fields were collected"),
        };
        Ok(Step::Parameters(next))
    }

    pub(super) fn rejected(&mut self, message: &str) -> Result<(), WizardFailure> {
        self.interaction.show(&Notice::ValidationFailed {
            message: message.into(),
        })?;
        Ok(())
    }
}

impl Draft {
    fn set(&mut self, field: Field, text: String) -> bool {
        match field {
            Field::Ips => match text
                .split(',')
                .map(|ip| ip.trim().parse::<IpAddr>())
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(ips) if !ips.is_empty() => {
                    self.ips = Some(ips);
                    true
                }
                _ => false,
            },
            Field::Port => match text.parse::<u16>() {
                Ok(port) if port >= 1024 => {
                    self.port = Some(port);
                    true
                }
                _ => false,
            },
            Field::Hostname => {
                self.hostname = Some(text);
                true
            }
            Field::Origin => {
                self.origin = Some(text);
                true
            }
            Field::ContestEnd | Field::GatewayExpiry => {
                if text.ends_with('Z') && OffsetDateTime::parse(&text, &Rfc3339).is_ok() {
                    if field == Field::ContestEnd {
                        self.contest_end = Some(text);
                    } else {
                        self.gateway_expiry = Some(text);
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(super) fn clear_from(&mut self, field: Field) {
        match field {
            Field::Ips => {
                self.ips = None;
                self.clear_from(Field::Port);
            }
            Field::Port => {
                self.port = None;
                self.clear_from(Field::Hostname);
            }
            Field::Hostname => {
                self.hostname = None;
                self.clear_from(Field::Origin);
            }
            Field::Origin => {
                self.origin = None;
                self.clear_from(Field::ContestEnd);
            }
            Field::ContestEnd => {
                self.contest_end = None;
                self.clear_from(Field::GatewayExpiry);
            }
            Field::GatewayExpiry => self.gateway_expiry = None,
        }
    }
}
