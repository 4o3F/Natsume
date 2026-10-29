//! Typed journal URL parsing without raw path or query reflection.
use crate::{
    component::device::DeviceId,
    journal::{
        event::{
            Event,
            types::{Category, Severity},
        },
        read::{PAGE_CAP, ReadRequest},
    },
    journal_query::{DEFAULT_PAGE, QueryError},
};
use uuid::Uuid;

pub(super) struct Window {
    /// Inclusive wall-clock lower bound, in Unix milliseconds.
    pub(super) from: i64,
    /// Inclusive wall-clock upper bound, in Unix milliseconds.
    pub(super) to: i64,
}
pub(super) fn events(raw: Option<&str>) -> Result<ReadRequest, QueryError> {
    let mut request = ReadRequest {
        limit: DEFAULT_PAGE,
        ..ReadRequest::default()
    };
    let mut seen = std::collections::HashSet::new();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        let repeated = matches!(
            key.as_ref(),
            "category[]" | "category" | "kind[]" | "kind" | "severity[]" | "severity"
        );
        if !repeated && !seen.insert(key.to_string()) {
            return Err(QueryError::Invalid);
        }
        match key.as_ref() {
            "category[]" | "category" => request.filter.categories.push(match value.as_ref() {
                "device" => Category::Device,
                "audit" => Category::Audit,
                "auth" => Category::Auth,
                "journal" => Category::Journal,
                _ => return Err(QueryError::Invalid),
            }),
            "severity[]" | "severity" => request.filter.severities.push(match value.as_ref() {
                "info" => Severity::Info,
                "warning" => Severity::Warning,
                "error" => Severity::Error,
                _ => return Err(QueryError::Invalid),
            }),
            "kind[]" | "kind" => {
                if Event::category_for_kind(&value).is_none() {
                    return Err(QueryError::Invalid);
                }
                request.filter.kinds.push(value.into_owned());
            }
            "device_id" => {
                request.filter.device_id =
                    Some(DeviceId::parse(&value).ok_or(QueryError::Invalid)?);
            }
            "actor_operator_id" => request.filter.actor_operator_id = Some(id(&value)?),
            "operation_id" => request.filter.operation_id = Some(id(&value)?),
            "request_id" => request.filter.request_id = Some(id(&value)?),
            "seat_prefix" => {
                if value.len() > 64 || value.chars().any(char::is_control) {
                    return Err(QueryError::Invalid);
                }
                request.filter.seat_prefix = Some(value.into_owned());
            }
            "from" => request.filter.from = Some(number(&value)?),
            "to" => request.filter.to = Some(number(&value)?),
            "before" => request.before = Some(cursor(&value)?),
            "after" => request.after = Some(cursor(&value)?),
            "limit" => {
                request.limit = u16::try_from(positive(&value)?)
                    .unwrap_or(PAGE_CAP)
                    .min(PAGE_CAP);
            }
            _ => return Err(QueryError::Invalid),
        }
        if request.filter.categories.len()
            + request.filter.kinds.len()
            + request.filter.severities.len()
            > 64
        {
            return Err(QueryError::Invalid);
        }
    }
    if request
        .filter
        .from
        .zip(request.filter.to)
        .is_some_and(|(from, to)| from > to)
        || request
            .before
            .zip(request.after)
            .is_some_and(|(before, after)| before <= after)
    {
        return Err(QueryError::Invalid);
    }
    Ok(request)
}
pub(super) fn window(raw: Option<&str>) -> Result<Window, QueryError> {
    if url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .any(|(key, _)| !matches!(key.as_ref(), "from" | "to"))
    {
        return Err(QueryError::Invalid);
    }
    let request = events(raw)?;
    Ok(Window {
        from: request.filter.from.ok_or(QueryError::Invalid)?,
        to: request.filter.to.ok_or(QueryError::Invalid)?,
    })
}
pub(super) fn summary_window(raw: Option<&str>) -> Result<i64, QueryError> {
    let mut result = None;
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        if key != "window_ms" || result.is_some() {
            return Err(QueryError::Invalid);
        }
        result = Some(positive(&value)?);
    }
    result.ok_or(QueryError::Invalid)
}
fn number(raw: &str) -> Result<i64, QueryError> {
    raw.parse().map_err(|source| QueryError::Number { source })
}
fn cursor(raw: &str) -> Result<i64, QueryError> {
    let value = number(raw)?;
    if value < 0 {
        return Err(QueryError::Invalid);
    }
    Ok(value)
}
fn positive(raw: &str) -> Result<i64, QueryError> {
    let value = number(raw)?;
    if value <= 0 {
        return Err(QueryError::Invalid);
    }
    Ok(value)
}
fn id(raw: &str) -> Result<Uuid, QueryError> {
    let id = Uuid::parse_str(raw).map_err(|source| QueryError::Id { source })?;
    if id.to_string() != raw {
        return Err(QueryError::Invalid);
    }
    Ok(id)
}
