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

use base64::Engine as _;
use sha2::{Digest, Sha256};
use snafu::Snafu;
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    audit::CorrelationId,
    db::{self, Database},
};

mod issuer;

#[cfg(test)]
pub(crate) use self::issuer::GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS;
use self::issuer::{
    CertificateSigningRequestDer, CertificateSigningRequestParams, raw_csr_spki_sha256,
};
pub(crate) use self::issuer::{GatewayIssuer, GatewayIssuerError, IssuedGatewayCertificate};
#[cfg(test)]
pub(crate) use self::issuer::{TEST_CONTEST_END, TEST_GATEWAY_HOSTNAME, TEST_GATEWAY_NOT_AFTER};
#[cfg(test)]
use self::issuer::{current_unix_seconds, encode_utc_timestamp};

pub(crate) const ENROLLMENT_PROTOCOL_VERSION: u32 = 1;
pub(crate) const MAX_GATEWAY_CSR_DER_BYTES: usize = 32 * 1024;
const MAX_CLIENT_VERSION_BYTES: usize = 64;
const DEVICE_TOKEN_BYTES: usize = 32;

pub(crate) trait DeviceConnectionEvictor: Send + Sync + 'static {
    fn evict_device_connection(&self, device_pk: &str) -> bool;
}

pub(crate) struct DeviceTokenAuthenticationFacts {
    pub(crate) device_pk: String,
    pub(crate) machine_hardware_id: String,
    pub(crate) token_hash: Vec<u8>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) struct NoLiveDeviceConnections;

#[cfg(test)]
impl DeviceConnectionEvictor for NoLiveDeviceConnections {
    fn evict_device_connection(&self, _device_pk: &str) -> bool {
        false
    }
}

#[derive(Clone)]
pub(crate) struct EnrollmentRequestInput {
    pub(crate) machine_hardware_id: String,
    pub(crate) hardware_identity_quality: String,
    pub(crate) gateway_csr_der: String,
    pub(crate) gateway_spki_sha256: String,
    pub(crate) client_version: String,
    pub(crate) protocol_version: u32,
}

pub(crate) struct ValidatedEnrollmentRequest {
    pub(crate) machine_hardware_id: String,
    pub(crate) hardware_identity_quality: HardwareIdentityQuality,
    pub(crate) gateway_csr_der: Vec<u8>,
    pub(crate) gateway_spki_sha256: [u8; 32],
    pub(crate) client_version: String,
    pub(crate) protocol_version: u32,
    pub(crate) source_ip: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardwareIdentityQuality {
    Strong,
    Medium,
    Weak,
}

impl HardwareIdentityQuality {
    fn parse(value: &str) -> Result<Self, EnrollmentError> {
        match value {
            "strong" => Ok(Self::Strong),
            "medium" => Ok(Self::Medium),
            "weak" => Ok(Self::Weak),
            _ => Err(EnrollmentError::InvalidHardwareIdentityQuality),
        }
    }

    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Medium => "medium",
            Self::Weak => "weak",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentResolution {
    CreateDevice,
    ReplaceDeviceCredentials,
}

impl EnrollmentResolution {
    fn from_persisted(value: &str) -> Result<Self, EnrollmentError> {
        match value {
            "create_device" => Ok(Self::CreateDevice),
            "replace_device_credentials" => Ok(Self::ReplaceDeviceCredentials),
            _ => Err(EnrollmentError::InvalidPersistedFacts),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentReviewState {
    Pending,
    Approved,
}

impl EnrollmentReviewState {
    fn from_persisted(value: &str) -> Result<Self, EnrollmentError> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            _ => Err(EnrollmentError::InvalidPersistedFacts),
        }
    }
}

/// Redacted live Enrollment facts exposed to authenticated operators.
pub(crate) struct EnrollmentRequestSummary {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) machine_hardware_id: Uuid,
    pub(crate) hardware_identity_quality: HardwareIdentityQuality,
    pub(crate) gateway_spki_sha256: String,
    pub(crate) client_version: String,
    pub(crate) protocol_version: u32,
    pub(crate) state: EnrollmentReviewState,
    pub(crate) resolution: Option<EnrollmentResolution>,
    pub(crate) resolved_device_id: Option<Uuid>,
    pub(crate) created_at: String,
    pub(crate) source_ip: String,
}

pub(crate) struct PersistedEnrollmentRequestSummary {
    pub(crate) enrollment_request_id: String,
    pub(crate) machine_hardware_id: String,
    pub(crate) hardware_identity_quality: String,
    pub(crate) gateway_spki_sha256: Vec<u8>,
    pub(crate) client_version: String,
    pub(crate) protocol_version: i64,
    pub(crate) state: String,
    pub(crate) resolution: Option<String>,
    pub(crate) resolved_device_id: Option<String>,
    pub(crate) created_at: String,
    pub(crate) source_ip: String,
}

impl EnrollmentRequestSummary {
    pub(crate) fn from_persisted(
        facts: PersistedEnrollmentRequestSummary,
    ) -> Result<Self, EnrollmentError> {
        let enrollment_request_id = parse_canonical_uuid(&facts.enrollment_request_id, 7)?;
        let machine_hardware_id = parse_canonical_uuid(&facts.machine_hardware_id, 5)?;
        if facts.gateway_spki_sha256.len() != 32
            || facts.client_version.is_empty()
            || !facts
                .client_version
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
            || facts.created_at.is_empty()
        {
            return Err(EnrollmentError::InvalidPersistedFacts);
        }
        let source_ip = facts
            .source_ip
            .parse::<IpAddr>()
            .map_err(|_| EnrollmentError::InvalidPersistedFacts)?
            .to_string();
        let resolved_device_id = facts
            .resolved_device_id
            .as_deref()
            .map(|value| parse_canonical_uuid(value, 7))
            .transpose()?;
        Ok(Self {
            enrollment_request_id,
            machine_hardware_id,
            hardware_identity_quality: HardwareIdentityQuality::parse(
                &facts.hardware_identity_quality,
            )
            .map_err(|_| EnrollmentError::InvalidPersistedFacts)?,
            gateway_spki_sha256: hex::encode(facts.gateway_spki_sha256),
            client_version: facts.client_version,
            protocol_version: u32::try_from(facts.protocol_version)
                .map_err(|_| EnrollmentError::InvalidPersistedFacts)?,
            state: EnrollmentReviewState::from_persisted(&facts.state)?,
            resolution: facts
                .resolution
                .as_deref()
                .map(EnrollmentResolution::from_persisted)
                .transpose()?,
            resolved_device_id,
            created_at: facts.created_at,
            source_ip,
        })
    }
}

fn parse_canonical_uuid(value: &str, version: usize) -> Result<Uuid, EnrollmentError> {
    let parsed = Uuid::parse_str(value).map_err(|_| EnrollmentError::InvalidPersistedFacts)?;
    if parsed.get_version_num() != version || parsed.hyphenated().to_string() != value {
        return Err(EnrollmentError::InvalidPersistedFacts);
    }
    Ok(parsed)
}

pub(crate) struct EnrollmentRequestId(Uuid);

impl EnrollmentRequestId {
    pub(crate) fn parse(value: &str) -> Result<Self, EnrollmentError> {
        let parsed = Uuid::parse_str(value).map_err(|_| EnrollmentError::InvalidRequestId)?;
        if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
            return Err(EnrollmentError::InvalidRequestId);
        }
        Ok(Self(parsed))
    }

    pub(crate) const fn value(&self) -> Uuid {
        self.0
    }

    #[cfg(test)]
    pub(crate) const fn for_test(value: Uuid) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentDecisionState {
    Approved,
    Rejected,
}

impl EnrollmentDecisionState {
    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

pub(crate) struct EnrollmentDecisionOutcome {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) state: EnrollmentDecisionState,
}

impl EnrollmentResolution {
    pub(crate) const fn as_persisted(self) -> &'static str {
        match self {
            Self::CreateDevice => "create_device",
            Self::ReplaceDeviceCredentials => "replace_device_credentials",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnrollmentState {
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssuanceReason {
    FirstEnrollment,
    CredentialReplacement,
    SameSpkiRetry,
}

impl IssuanceReason {
    pub(crate) const fn as_audit_reason(self) -> &'static str {
        match self {
            Self::FirstEnrollment => "first_enrollment",
            Self::CredentialReplacement => "credential_replacement",
            Self::SameSpkiRetry => "same_spki_retry",
        }
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct DeviceToken([u8; DEVICE_TOKEN_BYTES]);

impl DeviceToken {
    pub(crate) fn generate() -> Result<Self, EnrollmentError> {
        let mut bytes = [0_u8; DEVICE_TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| EnrollmentError::EntropyUnavailable)?;
        Ok(Self(bytes))
    }

    pub(crate) fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.0).into()
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; DEVICE_TOKEN_BYTES] {
        &self.0
    }
}

pub(crate) enum EnrollmentOutcome {
    Issued(IssuedEnrollment),
    Pending(PendingEnrollment),
}

pub(crate) struct IssuedEnrollment {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) device_id: Uuid,
    pub(crate) device_token: DeviceToken,
    pub(crate) gateway_leaf_der: Vec<u8>,
    pub(crate) gateway_chain_der: Vec<Vec<u8>>,
}

pub(crate) struct PendingEnrollment {
    pub(crate) enrollment_request_id: Uuid,
    pub(crate) state: EnrollmentState,
}

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
    db::enrollment::intake(
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
    db::enrollment::list_requests(database).await
}

pub(crate) async fn device_token_authentication_facts(
    database: &Database,
    token_hash: [u8; 32],
) -> Result<Option<DeviceTokenAuthenticationFacts>, EnrollmentError> {
    db::enrollment::device_token_authentication_facts(database, token_hash).await
}

pub(crate) async fn approve_request(
    database: &Database,
    request_id: &EnrollmentRequestId,
    correlation_id: CorrelationId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    db::enrollment::approve_request(database, request_id, correlation_id).await
}

pub(crate) async fn reject_request(
    database: &Database,
    request_id: &EnrollmentRequestId,
    correlation_id: CorrelationId,
) -> Result<EnrollmentDecisionOutcome, EnrollmentError> {
    db::enrollment::reject_request(database, request_id, correlation_id).await
}

fn validate_request(
    input: EnrollmentRequestInput,
    source_ip: IpAddr,
) -> Result<ValidatedEnrollmentRequest, EnrollmentError> {
    let machine_hardware_id = Uuid::parse_str(&input.machine_hardware_id)
        .map_err(|_| EnrollmentError::InvalidMachineHardwareId)?;
    if machine_hardware_id.get_version_num() != 5
        || machine_hardware_id.hyphenated().to_string() != input.machine_hardware_id
    {
        return Err(EnrollmentError::InvalidMachineHardwareId);
    }
    let hardware_identity_quality =
        HardwareIdentityQuality::parse(&input.hardware_identity_quality)?;
    if input.client_version.is_empty()
        || input.client_version.len() > MAX_CLIENT_VERSION_BYTES
        || !input
            .client_version
            .bytes()
            .all(|byte| byte.is_ascii_graphic())
    {
        return Err(EnrollmentError::InvalidClientVersion);
    }
    if input.protocol_version != ENROLLMENT_PROTOCOL_VERSION {
        return Err(EnrollmentError::UnsupportedProtocolVersion);
    }
    if input.gateway_spki_sha256.len() != 64
        || !input
            .gateway_spki_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EnrollmentError::InvalidSpki);
    }
    let mut claimed_spki = [0_u8; 32];
    hex::decode_to_slice(&input.gateway_spki_sha256, &mut claimed_spki)
        .map_err(|_| EnrollmentError::InvalidSpki)?;
    let gateway_csr_der = decode_standard_base64(&input.gateway_csr_der)
        .ok_or(EnrollmentError::InvalidCsrEncoding)?;
    if gateway_csr_der.is_empty() || gateway_csr_der.len() > MAX_GATEWAY_CSR_DER_BYTES {
        return Err(EnrollmentError::InvalidCsr);
    }
    CertificateSigningRequestParams::from_der(&CertificateSigningRequestDer::from(
        gateway_csr_der.as_slice(),
    ))
    .map_err(|_| EnrollmentError::InvalidCsr)?;
    let recomputed_spki =
        raw_csr_spki_sha256(&gateway_csr_der).map_err(|_| EnrollmentError::InvalidCsr)?;
    if !bool::from(recomputed_spki.ct_eq(&claimed_spki)) {
        return Err(EnrollmentError::SpkiMismatch);
    }
    Ok(ValidatedEnrollmentRequest {
        machine_hardware_id: input.machine_hardware_id,
        hardware_identity_quality,
        gateway_csr_der,
        gateway_spki_sha256: claimed_spki,
        client_version: input.client_version,
        protocol_version: input.protocol_version,
        source_ip: source_ip.to_string(),
    })
}

pub(crate) fn encode_standard_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_standard_base64(value: &str) -> Option<Vec<u8>> {
    // The empty-input and pre-decode length checks stay in front of the engine so the
    // encoding-vs-content error classes keep their frozen boundaries.
    let bytes = value.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = usize::from(bytes.ends_with(b"=")) + usize::from(bytes.ends_with(b"=="));
    let decoded_len = bytes
        .len()
        .checked_div(4)?
        .checked_mul(3)?
        .checked_sub(padding)?;
    if decoded_len > MAX_GATEWAY_CSR_DER_BYTES {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(bytes).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(crate) enum EnrollmentError {
    #[snafu(display("the Enrollment request ID is invalid"))]
    InvalidRequestId,
    #[snafu(display("the machine hardware ID is invalid"))]
    InvalidMachineHardwareId,
    #[snafu(display("the hardware identity quality is invalid"))]
    InvalidHardwareIdentityQuality,
    #[snafu(display("the client version is invalid"))]
    InvalidClientVersion,
    #[snafu(display("the Enrollment protocol version is unsupported"))]
    UnsupportedProtocolVersion,
    #[snafu(display("the claimed Gateway SPKI digest is invalid"))]
    InvalidSpki,
    #[snafu(display("the Gateway CSR encoding is invalid"))]
    InvalidCsrEncoding,
    #[snafu(display("the Gateway CSR is invalid"))]
    InvalidCsr,
    #[snafu(display("the claimed Gateway SPKI digest does not match the CSR"))]
    SpkiMismatch,
    #[snafu(display("the provisioning window is closed"))]
    ProvisioningWindowClosed,
    #[snafu(display("the Enrollment request was rejected"))]
    RequestRejected,
    #[snafu(display("the live Enrollment request capacity is exhausted"))]
    LiveRequestCapacityExceeded,
    #[snafu(display("the device identity conflicts with a live Enrollment request"))]
    DeviceIdentityConflict,
    #[snafu(display("the Enrollment request is not pending"))]
    RequestNotPending,
    #[snafu(display("the persisted Enrollment facts are invalid"))]
    InvalidPersistedFacts,
    #[snafu(display("Enrollment entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the Gateway issuance policy no longer has sufficient validity"))]
    IssuancePolicyExpired,
    #[snafu(display("Gateway certificate signing failed"))]
    SigningFailed,
    #[snafu(display("Enrollment persistence failed"))]
    PersistenceFailed,
}

impl From<GatewayIssuerError> for EnrollmentError {
    fn from(error: GatewayIssuerError) -> Self {
        match error {
            GatewayIssuerError::ValidityTooShort => Self::IssuancePolicyExpired,
            GatewayIssuerError::EntropyUnavailable => Self::EntropyUnavailable,
            GatewayIssuerError::MaterialUnreadable
            | GatewayIssuerError::InvalidMaterial
            | GatewayIssuerError::TrustRootUnreadable
            | GatewayIssuerError::InvalidTrustRoot
            | GatewayIssuerError::TrustRootMismatch
            | GatewayIssuerError::InvalidCsr
            | GatewayIssuerError::ClockInvalid
            | GatewayIssuerError::SigningFailed => Self::SigningFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, net::Ipv4Addr};

    use rcgen::{CertificateParams, KeyPair};
    use sha2::{Digest, Sha256};
    use snafu::Snafu;
    use x509_parser::{certification_request::X509CertificationRequest, prelude::FromDer as _};

    use crate::{
        config::{
            GatewaySiteConfig, ORIGIN_CA_CERTIFICATE_FILENAME, ORIGIN_CA_PRIVATE_KEY_FILENAME,
        },
        tls::tests::TestIdentity,
    };

    use super::{
        GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS, GatewayIssuer, GatewayIssuerError,
        current_unix_seconds, encode_utc_timestamp, raw_csr_spki_sha256,
    };

    #[test]
    fn csr_spki_digest_is_computed_over_the_raw_der_slice() -> Result<(), TestFailure> {
        let key = KeyPair::generate().map_err(|_| TestFailure::FixtureFailed)?;
        let csr = CertificateParams::new(vec!["ignored.invalid.example".to_owned()])
            .map_err(|_| TestFailure::FixtureFailed)?
            .serialize_request(&key)
            .map_err(|_| TestFailure::FixtureFailed)?;
        let (remainder, parsed) = X509CertificationRequest::from_der(csr.der())
            .map_err(|_| TestFailure::FixtureFailed)?;
        if !remainder.is_empty()
            || raw_csr_spki_sha256(csr.der()).map_err(|_| TestFailure::FixtureFailed)?
                != <[u8; 32]>::from(Sha256::digest(
                    parsed.certification_request_info.subject_pki.raw,
                ))
        {
            return Err(TestFailure::RawSpkiDigestChanged);
        }
        Ok(())
    }

    #[test]
    fn origin_ca_preflight_accepts_only_matching_der_material_and_valid_site_policy()
    -> Result<(), TestFailure> {
        let identity =
            TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let certificate_path = identity
            .directory_path()
            .join(ORIGIN_CA_CERTIFICATE_FILENAME);
        let private_key_path = identity
            .directory_path()
            .join(ORIGIN_CA_PRIVATE_KEY_FILENAME);
        let packaged_root_path = identity.directory_path().join("local-origin-ca.crt");
        let site = valid_site()?;
        GatewayIssuer::load(
            &certificate_path,
            &private_key_path,
            &packaged_root_path,
            site,
        )
        .map_err(|_| TestFailure::ExpectedValidMaterial)?;

        let other =
            TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let original_packaged_root =
            fs::read(&packaged_root_path).map_err(|_| TestFailure::FixtureFailed)?;
        fs::copy(
            other.directory_path().join("local-origin-ca.crt"),
            &packaged_root_path,
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        assert_preflight_error(
            GatewayIssuer::load(
                &certificate_path,
                &private_key_path,
                &packaged_root_path,
                valid_site()?,
            ),
            GatewayIssuerError::TrustRootMismatch,
        )?;
        fs::write(&packaged_root_path, original_packaged_root)
            .map_err(|_| TestFailure::FixtureFailed)?;
        fs::copy(
            other.directory_path().join(ORIGIN_CA_PRIVATE_KEY_FILENAME),
            &private_key_path,
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        assert_preflight_error(
            GatewayIssuer::load(
                &certificate_path,
                &private_key_path,
                &packaged_root_path,
                valid_site()?,
            ),
            GatewayIssuerError::InvalidMaterial,
        )?;

        fs::write(&private_key_path, b"malformed-origin-ca-key-canary")
            .map_err(|_| TestFailure::FixtureFailed)?;
        assert_preflight_error(
            GatewayIssuer::load(
                &certificate_path,
                &private_key_path,
                &packaged_root_path,
                valid_site()?,
            ),
            GatewayIssuerError::InvalidMaterial,
        )?;
        fs::copy(
            other.directory_path().join(ORIGIN_CA_PRIVATE_KEY_FILENAME),
            &private_key_path,
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        fs::write(&certificate_path, b"malformed-origin-ca-canary")
            .map_err(|_| TestFailure::FixtureFailed)?;
        assert_preflight_error(
            GatewayIssuer::load(
                &certificate_path,
                &private_key_path,
                &packaged_root_path,
                valid_site()?,
            ),
            GatewayIssuerError::InvalidMaterial,
        )?;
        fs::remove_file(&certificate_path).map_err(|_| TestFailure::FixtureFailed)?;
        assert_preflight_error(
            GatewayIssuer::load(
                &certificate_path,
                &private_key_path,
                &packaged_root_path,
                valid_site()?,
            ),
            GatewayIssuerError::MaterialUnreadable,
        )
    }

    #[test]
    fn issuance_time_margin_fails_closed_during_preflight() -> Result<(), TestFailure> {
        let identity =
            TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
        let now = current_unix_seconds().map_err(|_| TestFailure::FixtureFailed)?;
        let not_after = now
            .checked_add(GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS - 1)
            .ok_or(TestFailure::FixtureFailed)?;
        let contest_end = not_after
            .checked_sub(crate::config::GATEWAY_VALIDITY_MARGIN_SECONDS)
            .ok_or(TestFailure::FixtureFailed)?;
        let site = GatewaySiteConfig::for_test(
            "gateway.contest.example",
            &encode_utc_timestamp(not_after).map_err(|_| TestFailure::FixtureFailed)?,
            &encode_utc_timestamp(contest_end).map_err(|_| TestFailure::FixtureFailed)?,
        )
        .map_err(|_| TestFailure::FixtureFailed)?;
        assert_preflight_error(
            GatewayIssuer::load(
                &identity
                    .directory_path()
                    .join(ORIGIN_CA_CERTIFICATE_FILENAME),
                &identity
                    .directory_path()
                    .join(ORIGIN_CA_PRIVATE_KEY_FILENAME),
                &identity.directory_path().join("local-origin-ca.crt"),
                site,
            ),
            GatewayIssuerError::ValidityTooShort,
        )
    }

    fn valid_site() -> Result<GatewaySiteConfig, TestFailure> {
        GatewaySiteConfig::for_test(
            "gateway.contest.example",
            "4090-01-01T00:00:00Z",
            "4089-12-31T00:00:00Z",
        )
        .map_err(|_| TestFailure::FixtureFailed)
    }

    fn assert_preflight_error(
        result: Result<GatewayIssuer, GatewayIssuerError>,
        expected: GatewayIssuerError,
    ) -> Result<(), TestFailure> {
        let error = result.err().ok_or(TestFailure::ExpectedFailure)?;
        let display = error.to_string();
        let debug = format!("{error:?}");
        let canary_escaped = [
            "malformed-origin-ca-canary",
            "malformed-origin-ca-key-canary",
        ]
        .iter()
        .any(|canary| contains_canary(&display, canary) || contains_canary(&debug, canary));
        if error != expected || canary_escaped {
            return Err(TestFailure::UnexpectedFailure);
        }
        Ok(())
    }

    fn contains_canary(value: &str, canary: &str) -> bool {
        value
            .as_bytes()
            .windows(canary.len())
            .any(|window| window == canary.as_bytes())
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the Origin CA fixture failed"))]
        FixtureFailed,
        #[snafu(display("valid Origin CA material was rejected"))]
        ExpectedValidMaterial,
        #[snafu(display("an Origin CA preflight failure was expected"))]
        ExpectedFailure,
        #[snafu(display("the Origin CA preflight failure changed"))]
        UnexpectedFailure,
        #[snafu(display("the CSR SPKI digest no longer uses the raw DER slice"))]
        RawSpkiDigestChanged,
    }
}
