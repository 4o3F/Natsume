use natsume_workstation_protocol::{
    generated::{BindingAccessTarget, ForegroundTarget, ServerStateSnapshot},
    is_valid_domjudge_username,
};
use snafu::{OptionExt as _, ensure};

use crate::{
    connection::uuid_v7,
    error::{self, Result},
};

pub(crate) fn validate(wire: &ServerStateSnapshot) -> Result<()> {
    let target = wire.target.as_ref().context(error::PeerSnafu {
        code: "TARGET_MISSING",
    })?;
    let intent = wire.intent.as_ref().context(error::PeerSnafu {
        code: "INTENT_MISSING",
    })?;
    let gateway = target.gateway.as_ref().context(error::PeerSnafu {
        code: "GATEWAY_MISSING",
    })?;
    let session = target.session_control.as_ref().context(error::PeerSnafu {
        code: "SESSION_MISSING",
    })?;
    let home = target.home.as_ref().context(error::PeerSnafu {
        code: "HOME_MISSING",
    })?;
    let power = target.power.as_ref().context(error::PeerSnafu {
        code: "POWER_MISSING",
    })?;
    let runtime = target.runtime_config.as_ref().context(error::PeerSnafu {
        code: "RUNTIME_MISSING",
    })?;
    let binding = target.binding_access.as_ref().context(error::PeerSnafu {
        code: "BINDING_MISSING",
    })?;
    ensure!(
        uuid_v7(&gateway.credential_id)
            && intent
                .gateway_credential
                .as_ref()
                .is_some_and(|i| i.credential_id == gateway.credential_id)
            && matches!(
                ForegroundTarget::try_from(session.foreground_target),
                Ok(ForegroundTarget::Waiting | ForegroundTarget::Contest)
            )
            && [
                session.terminate_epoch,
                home.reset_epoch,
                power.shutdown_epoch
            ]
            .into_iter()
            .flatten()
            .all(epoch)
            && power.shutdown_epoch.is_some() == power.expires_at_unix_ms.is_some()
            && power.expires_at_unix_ms.is_none_or(|deadline| deadline > 0)
            && canonical_origin(&runtime.domjudge_origin),
        error::PeerSnafu {
            code: "TARGET_INVALID"
        }
    );
    if let Some(intent) = &intent.binding {
        ensure!(
            uuid_v7(&intent.negotiation_id)
                && binding.bound.is_none()
                && intent
                    .evaluation
                    .as_ref()
                    .is_none_or(|e| epoch(e.submission_epoch)
                        && matches!(
                            e.error_code.as_str(),
                            "SEAT_NOT_FOUND" | "SEAT_UNMAPPED" | "SEAT_OCCUPIED"
                        )),
            error::PeerSnafu {
                code: "BINDING_INTENT_INVALID"
            }
        );
    } else {
        ensure!(
            binding.bound.is_some(),
            error::PeerSnafu {
                code: "BINDING_INTENT_MISSING"
            }
        );
    }
    validate_binding(binding)
}

fn validate_binding(binding: &BindingAccessTarget) -> Result<()> {
    let Some(bound) = &binding.bound else {
        return Ok(());
    };
    let context = bound.context.as_ref().context(error::PeerSnafu {
        code: "BINDING_CONTEXT_MISSING",
    })?;
    let presentation = bound.presentation.as_ref().context(error::PeerSnafu {
        code: "PRESENTATION_MISSING",
    })?;
    let school = presentation
        .organization_id
        .strip_prefix("INST-")
        .unwrap_or_default();
    ensure!(
        uuid_v7(&context.binding_id)
            && uuid_v7(&context.account_id)
            && text(&context.seat_code, 64)
            && is_valid_domjudge_username(&context.domjudge_username)
            && epoch(context.credential_revision)
            && bound
                .password
                .as_ref()
                .and_then(|p| std::str::from_utf8(&p.value).ok())
                .is_some_and(|p| text(p, 512))
            && !school.is_empty()
            && school.len() >= 3
            && school.bytes().all(|c| c.is_ascii_digit())
            && school
                .parse::<u64>()
                .is_ok_and(|id| id > 0 && format!("INST-{id:03}") == presentation.organization_id)
            && names(&presentation.team_name_zh, &presentation.team_name_en)
            && names(&presentation.school_name_zh, &presentation.school_name_en),
        error::PeerSnafu {
            code: "BOUND_TARGET_INVALID"
        }
    );
    Ok(())
}

fn names(chinese: &str, english: &str) -> bool {
    (!chinese.is_empty() || !english.is_empty())
        && [chinese, english]
            .into_iter()
            .all(|s| s.len() <= 1024 && !s.chars().any(char::is_control))
}
fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn epoch(value: u64) -> bool {
    value > 0 && value <= i64::MAX.cast_unsigned()
}

pub(crate) fn canonical_origin(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.origin().ascii_serialization() == value
    })
}
