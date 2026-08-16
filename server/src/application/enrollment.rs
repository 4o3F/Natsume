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

use std::{fs, net::IpAddr, path::Path, sync::Arc, time::SystemTime};

use base64::Engine as _;
use rcgen::{
    CertificateParams, CertificateSigningRequestParams, DistinguishedName, ExtendedKeyUsagePurpose,
    IsCa, Issuer, KeyPair, KeyUsagePurpose, PublicKeyData, SerialNumber, SubjectPublicKeyInfo,
};
use rustls::{
    RootCertStore,
    client::{WebPkiServerVerifier, danger::ServerCertVerifier as _},
    server::ParsedCertificate,
};
use rustls_pki_types::{
    CertificateDer, CertificateSigningRequestDer, PrivatePkcs8KeyDer, ServerName, UnixTime,
    pem::PemObject as _,
};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use subtle::ConstantTimeEq;
use uuid::Uuid;
use x509_parser::{certification_request::X509CertificationRequest, prelude::FromDer as _};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    audit::CorrelationId,
    config::{GatewaySiteConfig, UtcDateTimeComponents},
    db::{self, Database},
    tls,
};

pub(crate) const ENROLLMENT_PROTOCOL_VERSION: u32 = 1;
pub(crate) const GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS: i64 = 300;
pub(crate) const MAX_GATEWAY_CSR_DER_BYTES: usize = 32 * 1024;
const MAX_CLIENT_VERSION_BYTES: usize = 64;
const DEVICE_TOKEN_BYTES: usize = 32;
const CERTIFICATE_SERIAL_BYTES: usize = 20;
#[cfg(test)]
pub(crate) const TEST_GATEWAY_HOSTNAME: &str = "gateway.contest.example";
#[cfg(test)]
pub(crate) const TEST_GATEWAY_NOT_AFTER: &str = "4090-01-01T00:00:00Z";
#[cfg(test)]
pub(crate) const TEST_CONTEST_END: &str = "4089-12-31T00:00:00Z";

#[derive(Clone)]
pub(crate) struct GatewayIssuer(Arc<GatewayIssuerInner>);

struct GatewayIssuerInner {
    issuer: Issuer<'static, KeyPair>,
    origin_certificate_der: Vec<u8>,
    site: GatewaySiteConfig,
}

impl GatewayIssuer {
    /// Loads and validates fixed-encoding Origin CA material and the site-owned
    /// Gateway profile before the HTTP listener is bound.
    pub(crate) fn load(
        certificate_path: &Path,
        private_key_path: &Path,
        packaged_origin_root_path: &Path,
        site: GatewaySiteConfig,
    ) -> Result<Self, GatewayIssuerError> {
        let certificate_bytes = tls::read_private_file(certificate_path)
            .map_err(|_| GatewayIssuerError::MaterialUnreadable)?;
        let mut private_key_bytes = tls::read_private_file(private_key_path)
            .map_err(|_| GatewayIssuerError::MaterialUnreadable)?;
        let private_key_der = PrivatePkcs8KeyDer::from(private_key_bytes.as_slice());
        let key_pair = KeyPair::try_from(&private_key_der).map_err(|_| {
            private_key_bytes.zeroize();
            GatewayIssuerError::InvalidMaterial
        })?;
        private_key_bytes.zeroize();

        let certificate_der = CertificateDer::from(certificate_bytes.clone());
        let parsed = ParsedCertificate::try_from(&certificate_der)
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        let packaged_origin_root = read_packaged_origin_root(packaged_origin_root_path)?;
        if certificate_der.as_ref() != packaged_origin_root.as_ref() {
            return Err(GatewayIssuerError::TrustRootMismatch);
        }
        if parsed.subject_public_key_info().as_ref()
            != key_pair.subject_public_key_info().as_slice()
        {
            return Err(GatewayIssuerError::InvalidMaterial);
        }
        let issuer: Issuer<'static, KeyPair> = Issuer::from_ca_cert_der(&certificate_der, key_pair)
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        let gateway_issuer = Self(Arc::new(GatewayIssuerInner {
            issuer,
            origin_certificate_der: certificate_bytes,
            site,
        }));
        gateway_issuer.verify_issuing_material()?;
        Ok(gateway_issuer)
    }

    fn verify_issuing_material(&self) -> Result<(), GatewayIssuerError> {
        let probe_key = KeyPair::generate().map_err(|_| GatewayIssuerError::EntropyUnavailable)?;
        let probe = self.sign_public_key(&probe_key)?;
        let origin = CertificateDer::from(self.0.origin_certificate_der.clone());
        let mut roots = RootCertStore::empty();
        roots
            .add(origin)
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        let server_name = ServerName::try_from(self.0.site.gateway_hostname().to_owned())
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        verifier
            .verify_server_cert(
                &CertificateDer::from(probe.leaf_der),
                &[],
                &server_name,
                &[],
                UnixTime::now(),
            )
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        Ok(())
    }

    pub(crate) fn issue_from_csr(
        &self,
        csr_der: &[u8],
    ) -> Result<IssuedGatewayCertificate, GatewayIssuerError> {
        CertificateSigningRequestParams::from_der(&CertificateSigningRequestDer::from(csr_der))
            .map_err(|_| GatewayIssuerError::InvalidCsr)?;
        let public_key = SubjectPublicKeyInfo::from_der(raw_csr_spki_der(csr_der)?)
            .map_err(|_| GatewayIssuerError::InvalidCsr)?;
        self.sign_public_key(&public_key)
    }

    fn sign_public_key(
        &self,
        public_key: &impl PublicKeyData,
    ) -> Result<IssuedGatewayCertificate, GatewayIssuerError> {
        let now = current_unix_seconds()?;
        if !self.0.site.has_required_validity_coverage() {
            return Err(GatewayIssuerError::ValidityTooShort);
        }
        let minimum_not_after = now
            .checked_add(GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS)
            .ok_or(GatewayIssuerError::ClockInvalid)?;
        if self.0.site.gateway_not_after().unix_seconds() < minimum_not_after {
            return Err(GatewayIssuerError::ValidityTooShort);
        }

        let mut serial_bytes = [0_u8; CERTIFICATE_SERIAL_BYTES];
        getrandom::fill(&mut serial_bytes).map_err(|_| GatewayIssuerError::EntropyUnavailable)?;
        serial_bytes[0] &= 0x7f;
        if serial_bytes.iter().all(|byte| *byte == 0) {
            serial_bytes[CERTIFICATE_SERIAL_BYTES - 1] = 1;
        }
        let serial = hex::encode(serial_bytes);
        let mut params = CertificateParams::new(vec![self.0.site.gateway_hostname().to_owned()])
            .map_err(|_| GatewayIssuerError::SigningFailed)?;
        params.distinguished_name = DistinguishedName::new();
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.serial_number = Some(SerialNumber::from_slice(&serial_bytes));
        set_certificate_time(
            &mut params,
            UtcDateTimeComponents::from_unix_seconds(now)
                .ok_or(GatewayIssuerError::ClockInvalid)?,
            CertificateTimeField::NotBefore,
        )?;
        set_certificate_time(
            &mut params,
            self.0.site.gateway_not_after().components(),
            CertificateTimeField::NotAfter,
        )?;
        let certificate = params
            .signed_by(public_key, &self.0.issuer)
            .map_err(|_| GatewayIssuerError::SigningFailed)?;
        Ok(IssuedGatewayCertificate {
            leaf_der: certificate.der().to_vec(),
            chain_der: vec![self.0.origin_certificate_der.clone()],
            serial,
            not_after: self.0.site.gateway_not_after().encoded().to_owned(),
        })
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn for_test() -> Result<Self, GatewayIssuerError> {
        let site = GatewaySiteConfig::for_test(
            TEST_GATEWAY_HOSTNAME,
            TEST_GATEWAY_NOT_AFTER,
            TEST_CONTEST_END,
        )
        .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        Self::for_test_with_site(site)
    }

    /// Bypasses startup preflight so tests can exercise the issuance-time
    /// validity recheck that protects a long-running process.
    #[cfg(test)]
    pub(crate) fn for_test_with_remaining_validity(
        remaining_seconds: i64,
    ) -> Result<Self, GatewayIssuerError> {
        let not_after = current_unix_seconds()?
            .checked_add(remaining_seconds)
            .and_then(UtcDateTimeComponents::from_unix_seconds)
            .ok_or(GatewayIssuerError::ClockInvalid)?;
        let contest_end = current_unix_seconds()?
            .checked_add(remaining_seconds)
            .and_then(|value| value.checked_sub(crate::config::GATEWAY_VALIDITY_MARGIN_SECONDS))
            .and_then(UtcDateTimeComponents::from_unix_seconds)
            .ok_or(GatewayIssuerError::ClockInvalid)?;
        let encoded_not_after = encode_utc_timestamp(not_after);
        let encoded_contest_end = encode_utc_timestamp(contest_end);
        let site = GatewaySiteConfig::for_test(
            TEST_GATEWAY_HOSTNAME,
            &encoded_not_after,
            &encoded_contest_end,
        )
        .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        Self::for_test_with_site(site)
    }

    #[cfg(test)]
    fn for_test_with_site(site: GatewaySiteConfig) -> Result<Self, GatewayIssuerError> {
        use rcgen::{BasicConstraints, CertificateParams, IsCa};

        let key = KeyPair::generate().map_err(|_| GatewayIssuerError::EntropyUnavailable)?;
        let mut params = CertificateParams::new(Vec::<String>::new())
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        params.distinguished_name = DistinguishedName::new();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        params.not_before = rcgen::date_time_ymd(2020, 1, 1);
        params.not_after = rcgen::date_time_ymd(4095, 1, 1);
        let certificate = params
            .self_signed(&key)
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        let certificate_der = CertificateDer::from(certificate.der().to_vec());
        let issuer = Issuer::from_ca_cert_der(&certificate_der, key)
            .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        Ok(Self(Arc::new(GatewayIssuerInner {
            issuer,
            origin_certificate_der: certificate.der().to_vec(),
            site,
        })))
    }
}

fn read_packaged_origin_root(path: &Path) -> Result<CertificateDer<'static>, GatewayIssuerError> {
    let encoded = fs::read(path).map_err(|_| GatewayIssuerError::TrustRootUnreadable)?;
    let mut certificates = CertificateDer::pem_slice_iter(&encoded);
    let certificate = certificates
        .next()
        .ok_or(GatewayIssuerError::InvalidTrustRoot)?
        .map_err(|_| GatewayIssuerError::InvalidTrustRoot)?;
    if certificates.next().is_some() {
        return Err(GatewayIssuerError::InvalidTrustRoot);
    }
    ParsedCertificate::try_from(&certificate).map_err(|_| GatewayIssuerError::InvalidTrustRoot)?;
    Ok(certificate)
}

fn raw_csr_spki_der(csr_der: &[u8]) -> Result<&[u8], GatewayIssuerError> {
    let (remainder, csr) =
        X509CertificationRequest::from_der(csr_der).map_err(|_| GatewayIssuerError::InvalidCsr)?;
    if !remainder.is_empty() {
        return Err(GatewayIssuerError::InvalidCsr);
    }
    Ok(csr.certification_request_info.subject_pki.raw)
}

fn raw_csr_spki_sha256(csr_der: &[u8]) -> Result<[u8; 32], GatewayIssuerError> {
    Ok(Sha256::digest(raw_csr_spki_der(csr_der)?).into())
}

#[cfg(test)]
fn encode_utc_timestamp(components: UtcDateTimeComponents) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        components.year,
        components.month,
        components.day,
        components.hour,
        components.minute,
        components.second
    )
}

fn current_unix_seconds() -> Result<i64, GatewayIssuerError> {
    let elapsed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| GatewayIssuerError::ClockInvalid)?;
    i64::try_from(elapsed.as_secs()).map_err(|_| GatewayIssuerError::ClockInvalid)
}

#[derive(Clone, Copy)]
enum CertificateTimeField {
    NotBefore,
    NotAfter,
}

fn set_certificate_time(
    params: &mut CertificateParams,
    components: UtcDateTimeComponents,
    field: CertificateTimeField,
) -> Result<(), GatewayIssuerError> {
    let timestamp = rcgen::date_time_ymd(components.year, components.month, components.day)
        .replace_hour(components.hour)
        .and_then(|value| value.replace_minute(components.minute))
        .and_then(|value| value.replace_second(components.second))
        .and_then(|value| value.replace_nanosecond(components.nanosecond))
        .map_err(|_| GatewayIssuerError::ClockInvalid)?;
    match field {
        CertificateTimeField::NotBefore => params.not_before = timestamp,
        CertificateTimeField::NotAfter => params.not_after = timestamp,
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
#[snafu(module)]
pub(crate) enum GatewayIssuerError {
    #[snafu(display("the Origin CA material could not be read"))]
    MaterialUnreadable,
    #[snafu(display("the Origin CA material is invalid"))]
    InvalidMaterial,
    #[snafu(display("the packaged Origin CA trust root could not be read"))]
    TrustRootUnreadable,
    #[snafu(display("the packaged Origin CA trust root is invalid"))]
    InvalidTrustRoot,
    #[snafu(display("the packaged Origin CA trust root does not match the issuing certificate"))]
    TrustRootMismatch,
    #[snafu(display("the Gateway CSR is invalid"))]
    InvalidCsr,
    #[snafu(display("the system clock is invalid"))]
    ClockInvalid,
    #[snafu(display("the Gateway certificate validity policy is too short"))]
    ValidityTooShort,
    #[snafu(display("certificate entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the Gateway certificate could not be signed"))]
    SigningFailed,
}

pub(crate) struct IssuedGatewayCertificate {
    pub(crate) leaf_der: Vec<u8>,
    pub(crate) chain_der: Vec<Vec<u8>>,
    pub(crate) serial: String,
    pub(crate) not_after: String,
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
pub(crate) async fn intake(
    database: &Database,
    issuer: GatewayIssuer,
    input: EnrollmentRequestInput,
    source_ip: IpAddr,
    correlation_id: CorrelationId,
) -> Result<EnrollmentOutcome, EnrollmentError> {
    let request = validate_request(input, source_ip)?;
    db::enrollment::intake(database, issuer, request, correlation_id).await
}

/// Reads all live (`pending` / `approved`) requests in stable creation order.
pub(crate) async fn list_requests(
    database: &Database,
) -> Result<Vec<EnrollmentRequestSummary>, EnrollmentError> {
    db::enrollment::list_requests(database).await
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
            UtcDateTimeComponents,
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
            .and_then(UtcDateTimeComponents::from_unix_seconds)
            .ok_or(TestFailure::FixtureFailed)?;
        let contest_end = now
            .checked_add(GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS - 1)
            .and_then(|value| value.checked_sub(crate::config::GATEWAY_VALIDITY_MARGIN_SECONDS))
            .and_then(UtcDateTimeComponents::from_unix_seconds)
            .ok_or(TestFailure::FixtureFailed)?;
        let site = GatewaySiteConfig::for_test(
            "gateway.contest.example",
            &encode_utc_timestamp(not_after),
            &encode_utc_timestamp(contest_end),
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
