use super::{Host, Interaction, Step, Wizard, WizardFailure, value};
use crate::{
    config::{ConfigRenderError, DeploymentParameters, ParameterRejection},
    interaction::{
        PromptId,
        notice::{self, LocalTimeSummary, Notice},
    },
};
use std::net::IpAddr;
use time::{Date, OffsetDateTime, Time, format_description::well_known::Rfc3339};

/// Parameter fields in forward navigation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Field {
    Ips,
    Port,
    Hostname,
    Origin,
    /// Selects the local competition completion date.
    ContestEndDate,
    /// Collects the local competition completion time.
    ContestEndTime,
    /// Selects the local Gateway expiry date.
    GatewayExpiryDate,
    /// Collects the local Gateway expiry time.
    GatewayExpiryTime,
}

impl Field {
    pub(super) const fn prompt(self) -> PromptId {
        match self {
            Self::Ips => PromptId::ServerIps,
            Self::Port => PromptId::HttpsPort,
            Self::Hostname => PromptId::GatewayHostname,
            Self::Origin => PromptId::DomjudgeOrigin,
            Self::ContestEndDate => PromptId::ContestEndDate,
            Self::ContestEndTime => PromptId::ContestEndTime,
            Self::GatewayExpiryDate => PromptId::GatewayNotAfterDate,
            Self::GatewayExpiryTime => PromptId::GatewayNotAfterTime,
        }
    }
    pub(super) const fn previous(self) -> Option<Self> {
        match self {
            Self::Ips => None,
            Self::Port => Some(Self::Ips),
            Self::Hostname => Some(Self::Port),
            Self::Origin => Some(Self::Hostname),
            Self::ContestEndDate => Some(Self::Origin),
            Self::ContestEndTime => Some(Self::ContestEndDate),
            Self::GatewayExpiryDate => Some(Self::ContestEndTime),
            Self::GatewayExpiryTime => Some(Self::GatewayExpiryDate),
        }
    }
}

#[derive(Debug, Default)]
struct Deadline {
    date: Option<Date>,
    time: Option<Time>,
    utc: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct Draft {
    ips: Option<Vec<IpAddr>>,
    port: Option<u16>,
    hostname: Option<String>,
    origin: Option<String>,
    contest_end: Deadline,
    gateway_expiry: Deadline,
}

pub(super) const fn rejection_prompt(reason: ParameterRejection) -> PromptId {
    match reason {
        ParameterRejection::EmptyServerIps
        | ParameterRejection::TooManyServerIps
        | ParameterRejection::UnusableServerIp => PromptId::ServerIps,
        ParameterRejection::HttpsPortOutOfRange => PromptId::HttpsPort,
        ParameterRejection::EmptyGatewayHostname => PromptId::GatewayHostname,
        ParameterRejection::EmptyDomjudgeOrigin => PromptId::DomjudgeOrigin,
        ParameterRejection::ContestEndFormat => PromptId::ContestEndDate,
        ParameterRejection::GatewayNotAfterFormat
        | ParameterRejection::GatewayValidityTooShort
        | ParameterRejection::GatewayExpired => PromptId::GatewayNotAfterDate,
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
        PromptId::ContestEndDate => Field::ContestEndDate,
        _ => Field::GatewayExpiryDate,
    })
}

fn parse_time(text: &str) -> Option<Time> {
    let (hour, minute) = text.split_once(':')?;
    if hour.len() != 2
        || minute.len() != 2
        || !hour
            .bytes()
            .chain(minute.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Time::from_hms(hour.parse().ok()?, minute.parse().ok()?, 0).ok()
}

impl<I: Interaction, H: Host> Wizard<'_, I, H> {
    pub(super) fn parameter(&mut self, field: Field) -> Result<Step, WizardFailure> {
        let id = field.prompt();
        match field {
            Field::ContestEndDate | Field::GatewayExpiryDate => {
                let today = self.host.utc_to_local(self.host.now())?.date();
                let (default, min) = if field == Field::ContestEndDate {
                    (today, today)
                } else {
                    let Some(end) = self.answers.draft.contest_end.date else {
                        unreachable!("contest date precedes Gateway expiry")
                    };
                    let Some(default) = end.next_day() else {
                        self.rejected(notice::INVALID_EXPIRY_DATE)?;
                        self.reset(Step::Parameters(Field::ContestEndDate));
                        return Ok(Step::Parameters(Field::ContestEndDate));
                    };
                    (default, end)
                };
                let date =
                    value(
                        self.interaction
                            .date(id, notice::prompt(id), default, Some(min))?,
                    )?;
                if field == Field::ContestEndDate {
                    self.answers.draft.contest_end.date = Some(date);
                } else {
                    self.answers.draft.gateway_expiry.date = Some(date);
                }
            }
            Field::ContestEndTime | Field::GatewayExpiryTime => {
                if !self.parameter_time(field)? {
                    return Ok(Step::Parameters(field));
                }
            }
            _ => {
                let default = (field == Field::Port)
                    .then(|| self.answers.draft.port.unwrap_or(8443).to_string());
                let text = value(self.interaction.text(
                    id,
                    notice::prompt(id),
                    default.as_deref(),
                    None,
                )?)?;
                if !self.answers.draft.set_text(field, text) {
                    self.rejected(if field == Field::Ips {
                        notice::INVALID_IP
                    } else {
                        notice::INVALID_PORT
                    })?;
                    return Ok(Step::Parameters(field));
                }
            }
        }
        let draft = &self.answers.draft;
        if let (Some(ips), Some(port), Some(hostname), Some(origin), Some(end), Some(expiry)) = (
            &draft.ips,
            draft.port,
            &draft.hostname,
            &draft.origin,
            &draft.contest_end.utc,
            &draft.gateway_expiry.utc,
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
                        if matches!(retry, Field::ContestEndDate | Field::GatewayExpiryDate) {
                            self.answers.draft.clear_from(retry);
                        }
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
            Field::Origin => Field::ContestEndDate,
            Field::ContestEndDate => Field::ContestEndTime,
            Field::ContestEndTime => Field::GatewayExpiryDate,
            Field::GatewayExpiryDate => Field::GatewayExpiryTime,
            Field::GatewayExpiryTime => unreachable!("all parameter fields were collected"),
        };
        Ok(Step::Parameters(next))
    }

    fn parameter_time(&mut self, field: Field) -> Result<bool, WizardFailure> {
        let id = field.prompt();
        let default = if field == Field::GatewayExpiryTime {
            self.answers
                .draft
                .contest_end
                .time
                .map(|time| format!("{:02}:{:02}", time.hour(), time.minute()))
        } else {
            None
        };
        let text =
            value(
                self.interaction
                    .text(id, notice::prompt(id), default.as_deref(), None)?,
            )?;
        let Some(time) = parse_time(&text) else {
            self.rejected(notice::INVALID_TIME)?;
            return Ok(false);
        };
        let deadline = if field == Field::ContestEndTime {
            &self.answers.draft.contest_end
        } else {
            &self.answers.draft.gateway_expiry
        };
        let Some(date) = deadline.date else {
            unreachable!("calendar date precedes time input")
        };
        let utc = match self.host.local_to_utc(date, time) {
            Ok(utc) => utc,
            Err(error) => {
                self.rejected(&snafu::Report::from_error(&error).to_string())?;
                return Ok(false);
            }
        };
        let timestamp = utc.format(&Rfc3339)?;
        let summary = self.local_time_summary(
            &timestamp,
            if field == Field::ContestEndTime {
                ParameterRejection::ContestEndFormat
            } else {
                ParameterRejection::GatewayNotAfterFormat
            },
        )?;
        self.interaction
            .show(&Notice::SelectedTime { field: id, summary })?;
        let deadline = if field == Field::ContestEndTime {
            &mut self.answers.draft.contest_end
        } else {
            &mut self.answers.draft.gateway_expiry
        };
        deadline.time = Some(time);
        deadline.utc = Some(timestamp);
        Ok(true)
    }

    pub(super) fn local_time_summary(
        &self,
        timestamp: &str,
        reason: ParameterRejection,
    ) -> Result<LocalTimeSummary, WizardFailure> {
        let utc = OffsetDateTime::parse(timestamp, &Rfc3339)
            .map_err(|source| ConfigRenderError::TimestampParse { reason, source })?;
        Ok(LocalTimeSummary {
            local: self.host.utc_to_local(utc)?,
            timezone: self.host.timezone(),
            utc: timestamp.into(),
        })
    }

    pub(super) fn rejected(&mut self, message: &str) -> Result<(), WizardFailure> {
        self.interaction.show(&Notice::ValidationFailed {
            message: message.into(),
        })?;
        Ok(())
    }
}

impl Draft {
    fn set_text(&mut self, field: Field, text: String) -> bool {
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
            _ => unreachable!("calendar fields use date and time prompts"),
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
                self.clear_from(Field::ContestEndDate);
            }
            Field::ContestEndDate => {
                self.contest_end.date = None;
                self.clear_from(Field::ContestEndTime);
            }
            Field::ContestEndTime => {
                self.contest_end.time = None;
                self.contest_end.utc = None;
                self.clear_from(Field::GatewayExpiryDate);
            }
            Field::GatewayExpiryDate => {
                self.gateway_expiry.date = None;
                self.clear_from(Field::GatewayExpiryTime);
            }
            Field::GatewayExpiryTime => {
                self.gateway_expiry.time = None;
                self.gateway_expiry.utc = None;
            }
        }
    }
}
