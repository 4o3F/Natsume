use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::types::{COMMAND_REQUEST_FINGERPRINT_DOMAIN, CommandError, CommandKind};

#[derive(Serialize)]
struct FingerprintInput<'a> {
    device_id: &'a str,
    kind: &'static str,
    payload_version: i32,
    payload: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_correlation_id: Option<&'a str>,
}

pub(in crate::application::command) fn fingerprint_v1(
    device_id: &str,
    kind: CommandKind,
    payload_version: i32,
    payload: &Value,
    reason_code: Option<&str>,
    group_correlation_id: Option<&str>,
) -> Result<Vec<u8>, CommandError> {
    let canonical = serde_json_canonicalizer::to_vec(&FingerprintInput {
        device_id,
        kind: kind.as_str(),
        payload_version,
        payload,
        reason_code,
        group_correlation_id,
    })
    .map_err(|_| CommandError::CanonicalizationFailed)?;
    let mut hasher = Sha256::new();
    hasher.update(COMMAND_REQUEST_FINGERPRINT_DOMAIN);
    hasher.update([0]);
    hasher.update(canonical);
    Ok(hasher.finalize().to_vec())
}
