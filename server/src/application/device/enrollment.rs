//! Enrollment intake, validation, and the single Gateway certificate profile.
//!
//! Same-SPKI replacement is issued immediately on the first eligible POST. The
//! CSR signature proves possession of the current private key, so persisting a
//! synthetic approval would add no authority and would make response-loss
//! recovery less direct. Different-SPKI replacement remains approve-then-claim.
//! A rejected request blocks the hardware identity while it remains that
//! identity's newest request; window close expires it and therefore clears the
//! block without adding a window identifier column. A non-pending operator
//! decision is classified as `ENROLLMENT_REQUEST_INVALID`, because the named
//! request exists but is no longer actionable. Approval/rejection repeats are
//! noops only when the persisted state already equals the requested target;
//! cross-target and terminal transitions use the same not-actionable class.

use std::net::IpAddr;

use crate::{
    audit::CorrelationId,
    db::{self, Database},
};

use super::credentials::DeviceConnectionEvictor;
#[cfg(test)]
use super::credentials::NoLiveDeviceConnections;

mod decision;
pub(in crate::application::device) mod identifier;
mod intake;
mod issuer;
mod types;
mod validate;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use self::issuer::GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS;
pub(crate) use self::issuer::{GatewayIssuer, GatewayIssuerError, IssuedGatewayCertificate};
#[cfg(test)]
pub(crate) use self::issuer::{TEST_CONTEST_END, TEST_GATEWAY_HOSTNAME, TEST_GATEWAY_NOT_AFTER};
#[cfg(test)]
use self::issuer::{current_unix_seconds, encode_utc_timestamp, raw_csr_spki_sha256};
pub(crate) use self::types::{
    ENROLLMENT_PROTOCOL_VERSION, EnrollmentDecisionOutcome, EnrollmentDecisionProjection,
    EnrollmentDecisionState, EnrollmentError, EnrollmentOutcome, EnrollmentRequestId,
    EnrollmentRequestInput, EnrollmentRequestStatus, EnrollmentRequestSummary,
    EnrollmentResolution, EnrollmentReviewState, EnrollmentState, IntakeIds, IssuanceReason,
    IssuedEnrollment, IssuedRequestMode, LatestEnrollmentRequestProjection,
    LiveEnrollmentRequestProjection, MAX_GATEWAY_CSR_DER_BYTES, MAX_LIVE_ENROLLMENT_REQUESTS,
    PendingEnrollment, ValidatedEnrollmentRequest,
};
pub(crate) use self::validate::encode_standard_base64;
use self::validate::validate_request;

/// Validates a device request completely before any database access, then lets
/// the store perform the window gate and state transition atomically.
#[cfg(test)]
pub(crate) async fn intake(
    database: &Database,
    issuer: GatewayIssuer,
    input: EnrollmentRequestInput,
    source_ip: IpAddr,
    correlation_id: CorrelationId,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    intake_with_connection_eviction(
        database,
        issuer,
        input,
        source_ip,
        correlation_id,
        NoLiveDeviceConnections,
    )
    .await
}

pub(crate) async fn intake_with_connection_eviction<E>(
    database: &Database,
    issuer: GatewayIssuer,
    input: EnrollmentRequestInput,
    source_ip: IpAddr,
    correlation_id: CorrelationId,
    connection_evictor: E,
) -> Result<EnrollmentOutcome, EnrollmentError>
where
    E: DeviceConnectionEvictor,
{
    let request = validate_request(input, source_ip)?;
    intake::intake_validated(
        database,
        issuer,
        request,
        correlation_id,
        connection_evictor,
    )
    .await
}

/// Reads all live (`pending` / `approved`) requests in stable creation order.
pub(crate) async fn list_requests(
    database: &Database,
) -> Result<Vec<EnrollmentRequestSummary>, EnrollmentError> {
    database
        .read(db::device::enrollment::query::list_live_requests)
        .await
}

pub(crate) async fn approve_request(
    database: &Database,
    request_id: &EnrollmentRequestId,
    correlation_id: CorrelationId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    decision::approve_request(database, request_id, correlation_id).await
}

pub(crate) async fn reject_request(
    database: &Database,
    request_id: &EnrollmentRequestId,
    correlation_id: CorrelationId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    decision::reject_request(database, request_id, correlation_id).await
}
