use std::{fs, path::Path};

#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use rcgen::{
    CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData, SanType, SerialNumber,
    SignatureAlgorithm,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer, pem::PemObject};
use snafu::Snafu;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use x509_parser::{
    certification_request::X509CertificationRequest,
    oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256},
    parse_x509_certificate,
    prelude::{FromDer, X509Version},
};
use zeroize::Zeroize;

use crate::{config::GatewaySiteConfig, tls::read_private_file};

const CLOCK_SKEW_ALLOWANCE: Duration = Duration::minutes(5);

/// Concrete Origin CA signer for the fixed Gateway leaf profile.
pub(in crate::component::gateway) struct GatewayIssuer {
    issuer: Issuer<'static, KeyPair>,
    gateway_hostname: String,
    gateway_not_after: OffsetDateTime,
    #[cfg(test)]
    issue_count: Arc<AtomicUsize>,
}

impl GatewayIssuer {
    /// Loads and validates the fixed Origin CA material.
    pub(in crate::component::gateway) fn load(
        ca_certificate_path: &Path,
        ca_private_key_path: &Path,
        packaged_trust_root_path: &Path,
        site: &GatewaySiteConfig,
    ) -> Result<Self, GatewayIssuerError> {
        let ca_certificate_der = CertificateDer::from(
            read_private_file(ca_certificate_path).map_err(|_| GatewayIssuerError::OriginCa)?,
        );
        let packaged_trust_root_der = parse_packaged_trust_root(
            fs::read(packaged_trust_root_path).map_err(|_| GatewayIssuerError::OriginCa)?,
        )?;
        let mut ca_private_key =
            read_private_file(ca_private_key_path).map_err(|_| GatewayIssuerError::OriginCa)?;
        let result = Self::new(
            &ca_certificate_der,
            &PrivatePkcs8KeyDer::from(ca_private_key.as_slice()),
            &packaged_trust_root_der,
            site,
        );
        ca_private_key.zeroize();
        result
    }

    fn new(
        ca_certificate_der: &CertificateDer<'_>,
        ca_private_key_der: &PrivatePkcs8KeyDer<'_>,
        packaged_trust_root_der: &CertificateDer<'_>,
        site: &GatewaySiteConfig,
    ) -> Result<Self, GatewayIssuerError> {
        if ca_certificate_der.as_ref() != packaged_trust_root_der.as_ref() {
            return Err(GatewayIssuerError::TrustRootMismatch);
        }

        let (remainder, ca_certificate) = parse_x509_certificate(ca_certificate_der.as_ref())
            .map_err(|_| GatewayIssuerError::OriginCa)?;
        if !remainder.is_empty()
            || !ca_certificate
                .basic_constraints()
                .map_err(|_| GatewayIssuerError::OriginCa)?
                .is_some_and(|constraints| constraints.value.ca)
            || ca_certificate
                .key_usage()
                .map_err(|_| GatewayIssuerError::OriginCa)?
                .is_some_and(|usage| !usage.value.key_cert_sign())
        {
            return Err(GatewayIssuerError::OriginCa);
        }

        let key_pair =
            KeyPair::try_from(ca_private_key_der).map_err(|_| GatewayIssuerError::OriginCa)?;
        if key_pair.public_key_raw()
            != ca_certificate
                .tbs_certificate
                .subject_pki
                .subject_public_key
                .data
                .as_ref()
        {
            return Err(GatewayIssuerError::OriginCa);
        }

        let issuer = Issuer::from_ca_cert_der(ca_certificate_der, key_pair)
            .map_err(|_| GatewayIssuerError::OriginCa)?;
        Ok(Self {
            issuer,
            gateway_hostname: site.gateway_hostname().to_owned(),
            gateway_not_after: site.gateway_not_after().timestamp(),
            #[cfg(test)]
            issue_count: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Verifies the exact PKCS#10 DER before it becomes an accepted durable fact.
    pub(in crate::component::gateway) fn validate_csr(
        csr_der: &[u8],
    ) -> Result<(), GatewayIssuerError> {
        parse_csr_public_key(csr_der).map(|_| ())
    }

    /// Signs the exact accepted CSR public key using the fixed Gateway leaf profile.
    pub(in crate::component::gateway) fn issue(
        &self,
        credential_id: Uuid,
        csr_der: &[u8],
    ) -> Result<IssuedGatewayCertificate, GatewayIssuerError> {
        let public_key = parse_csr_public_key(csr_der)?;
        let not_before = OffsetDateTime::now_utc()
            .checked_sub(CLOCK_SKEW_ALLOWANCE)
            .ok_or(GatewayIssuerError::IssuanceFailed)?;
        if self.gateway_not_after <= not_before {
            return Err(GatewayIssuerError::IssuanceFailed);
        }

        let mut params = CertificateParams::default();
        params.not_before = not_before;
        params.not_after = self.gateway_not_after;
        params.serial_number = Some(SerialNumber::from(credential_id.as_bytes().to_vec()));
        params.subject_alt_names = vec![SanType::DnsName(
            self.gateway_hostname
                .as_str()
                .try_into()
                .map_err(|_| GatewayIssuerError::IssuanceFailed)?,
        )];
        params.distinguished_name = DistinguishedName::new();
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

        #[cfg(test)]
        self.issue_count.fetch_add(1, Ordering::Relaxed);
        let leaf = params
            .signed_by(&public_key, &self.issuer)
            .map_err(|_| GatewayIssuerError::IssuanceFailed)?;
        Ok(IssuedGatewayCertificate {
            leaf_der: leaf.der().as_ref().to_vec(),
        })
    }
}

#[cfg(test)]
impl GatewayIssuer {
    pub(in crate::component::gateway) fn for_test()
    -> Result<(Self, Arc<AtomicUsize>), GatewayIssuerError> {
        use rcgen::BasicConstraints;

        let site = GatewaySiteConfig::for_test(
            "gateway.contest.example",
            "2099-01-02T00:00:00Z",
            "2099-01-01T00:00:00Z",
        )
        .map_err(|_| GatewayIssuerError::IssuanceFailed)?;
        let mut ca_params = CertificateParams::default();
        ca_params.distinguished_name = DistinguishedName::new();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca_key = KeyPair::generate().map_err(|_| GatewayIssuerError::IssuanceFailed)?;
        let ca_certificate = ca_params
            .self_signed(&ca_key)
            .map_err(|_| GatewayIssuerError::IssuanceFailed)?;
        let mut ca_private_key = ca_key.serialize_der();
        let ca_certificate_der = ca_certificate.der().clone();
        let result = Self::new(
            &ca_certificate_der,
            &PrivatePkcs8KeyDer::from(ca_private_key.as_slice()),
            &ca_certificate_der,
            &site,
        );
        ca_private_key.zeroize();
        result.map(|issuer| {
            let issue_count = Arc::clone(&issuer.issue_count);
            (issuer, issue_count)
        })
    }
}

/// Exact durable certificate bytes produced by one successful issuance.
pub(in crate::component::gateway) struct IssuedGatewayCertificate {
    leaf_der: Vec<u8>,
}

impl IssuedGatewayCertificate {
    pub(in crate::component::gateway) fn into_leaf_der(self) -> Vec<u8> {
        self.leaf_der
    }
}

/// Redacted Gateway issuer failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(in crate::component::gateway) enum GatewayIssuerError {
    #[snafu(display("Origin CA loading or validation failed"))]
    OriginCa,
    #[snafu(display("Origin CA issuing certificate and packaged trust root differ"))]
    TrustRootMismatch,
    #[snafu(display("Gateway CSR is invalid"))]
    InvalidCsr,
    #[snafu(display("Gateway certificate issuance failed"))]
    IssuanceFailed,
}

struct CsrPublicKey {
    raw: Vec<u8>,
}

impl PublicKeyData for CsrPublicKey {
    fn der_bytes(&self) -> &[u8] {
        &self.raw
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

fn parse_csr_public_key(csr_der: &[u8]) -> Result<CsrPublicKey, GatewayIssuerError> {
    let (remainder, csr) =
        X509CertificationRequest::from_der(csr_der).map_err(|_| GatewayIssuerError::InvalidCsr)?;
    if !remainder.is_empty()
        || csr.certification_request_info.version != X509Version::V1
        || csr.signature_algorithm.algorithm != OID_SIG_ECDSA_WITH_SHA256
    {
        return Err(GatewayIssuerError::InvalidCsr);
    }

    let subject_pki = &csr.certification_request_info.subject_pki;
    if subject_pki.algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY
        || subject_pki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|parameters| parameters.as_oid().ok())
            .is_none_or(|curve| curve != OID_EC_P256)
    {
        return Err(GatewayIssuerError::InvalidCsr);
    }
    csr.verify_signature()
        .map_err(|_| GatewayIssuerError::InvalidCsr)?;

    Ok(CsrPublicKey {
        raw: subject_pki.subject_public_key.data.to_vec(),
    })
}

fn parse_packaged_trust_root(
    encoded: Vec<u8>,
) -> Result<CertificateDer<'static>, GatewayIssuerError> {
    if parse_exact_certificate(&encoded) {
        return Ok(CertificateDer::from(encoded));
    }

    let certificates = CertificateDer::pem_slice_iter(&encoded)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| GatewayIssuerError::OriginCa)?;
    let [certificate] = certificates.as_slice() else {
        return Err(GatewayIssuerError::OriginCa);
    };
    Ok(certificate.clone())
}

fn parse_exact_certificate(der: &[u8]) -> bool {
    parse_x509_certificate(der).is_ok_and(|(remainder, _)| remainder.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::atomic::Ordering};

    use base64::Engine as _;
    use rcgen::{
        BasicConstraints, CustomExtension, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose, PKCS_ECDSA_P384_SHA384,
    };
    use snafu::Snafu;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;
    use x509_parser::{extensions::GeneralName, parse_x509_certificate};

    use super::{
        CertificateParams, DistinguishedName, GatewayIssuer, GatewayIssuerError, PrivatePkcs8KeyDer,
    };
    use crate::config::GatewaySiteConfig;

    const GATEWAY_HOSTNAME: &str = "gateway.contest.example";
    const GATEWAY_NOT_AFTER: &str = "2099-01-02T00:00:00Z";
    const CONTEST_END: &str = "2099-01-01T00:00:00Z";

    #[derive(Debug, Snafu)]
    enum TestError {
        #[snafu(display("test setup failed"))]
        Setup,
        #[snafu(display("test assertion failed"))]
        Assertion,
    }

    struct TestCa {
        certificate_der: rustls_pki_types::CertificateDer<'static>,
        private_key_der: Vec<u8>,
    }

    impl TestCa {
        fn generate() -> Result<Self, TestError> {
            let mut params = CertificateParams::default();
            params.distinguished_name = DistinguishedName::new();
            params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
            let key = KeyPair::generate().map_err(|_| TestError::Setup)?;
            let certificate = params.self_signed(&key).map_err(|_| TestError::Setup)?;
            Ok(Self {
                certificate_der: certificate.der().clone(),
                private_key_der: key.serialize_der(),
            })
        }
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Result<Self, TestError> {
            let path = std::env::temp_dir()
                .join(format!("natsume-gateway-issuer-test-{}", Uuid::now_v7()));
            fs::create_dir(&path).map_err(|_| TestError::Setup)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|_| TestError::Setup)?;
            Ok(Self { path })
        }

        fn write(&self, name: &str, contents: &[u8]) -> Result<PathBuf, TestError> {
            let path = self.path.join(name);
            fs::write(&path, contents).map_err(|_| TestError::Setup)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|_| TestError::Setup)?;
            Ok(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn site() -> Result<GatewaySiteConfig, TestError> {
        GatewaySiteConfig::for_test(GATEWAY_HOSTNAME, GATEWAY_NOT_AFTER, CONTEST_END)
            .map_err(|_| TestError::Setup)
    }

    fn issuer(ca: &TestCa) -> Result<GatewayIssuer, TestError> {
        GatewayIssuer::new(
            &ca.certificate_der,
            &PrivatePkcs8KeyDer::from(ca.private_key_der.as_slice()),
            &ca.certificate_der,
            &site()?,
        )
        .map_err(|_| TestError::Setup)
    }

    fn csr(params: &CertificateParams, key: &KeyPair) -> Result<Vec<u8>, TestError> {
        params
            .serialize_request(key)
            .map(|request| request.der().as_ref().to_vec())
            .map_err(|_| TestError::Setup)
    }

    #[test]
    fn issuance_uses_only_the_fixed_gateway_profile() -> Result<(), TestError> {
        let ca = TestCa::generate()?;
        let issuer = issuer(&ca)?;
        let mut requested = CertificateParams::new(vec!["attacker.example".to_owned()])
            .map_err(|_| TestError::Setup)?;
        requested
            .distinguished_name
            .push(DnType::CommonName, "attacker");
        requested.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        requested.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        requested.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        requested
            .custom_extensions
            .push(CustomExtension::from_oid_content(
                &[1, 3, 6, 1, 4, 1, 55555, 1],
                vec![5, 0],
            ));
        let subject_key = KeyPair::generate().map_err(|_| TestError::Setup)?;
        let csr_der = csr(&requested, &subject_key)?;
        let credential_id = Uuid::now_v7();
        let issued_at = OffsetDateTime::now_utc();

        GatewayIssuer::validate_csr(&csr_der).map_err(|_| TestError::Assertion)?;
        let leaf_der = issuer
            .issue(credential_id, &csr_der)
            .map_err(|_| TestError::Assertion)?
            .into_leaf_der();
        let (remainder, certificate) =
            parse_x509_certificate(&leaf_der).map_err(|_| TestError::Assertion)?;
        if !remainder.is_empty()
            || certificate.subject().iter().next().is_some()
            || certificate.raw_serial() != credential_id.as_bytes()
            || certificate.validity().not_after.to_datetime()
                != site()?.gateway_not_after().timestamp()
            || certificate.validity().not_before.to_datetime() < issued_at - Duration::minutes(6)
            || certificate.validity().not_before.to_datetime() > issued_at - Duration::minutes(4)
            || certificate
                .basic_constraints()
                .map_err(|_| TestError::Assertion)?
                .is_none_or(|constraints| constraints.value.ca)
            || certificate
                .key_usage()
                .map_err(|_| TestError::Assertion)?
                .is_none_or(|usage| !usage.value.digital_signature())
            || certificate
                .extended_key_usage()
                .map_err(|_| TestError::Assertion)?
                .is_none_or(|usage| !usage.value.server_auth || usage.value.client_auth)
        {
            return Err(TestError::Assertion);
        }
        let san = certificate
            .subject_alternative_name()
            .map_err(|_| TestError::Assertion)?
            .ok_or(TestError::Assertion)?;
        if san.value.general_names.as_slice() != [GeneralName::DNSName(GATEWAY_HOSTNAME)] {
            return Err(TestError::Assertion);
        }
        certificate
            .verify_signature(Some(ca_certificate(&ca)?.public_key()))
            .map_err(|_| TestError::Assertion)
    }

    #[test]
    fn malformed_tampered_and_non_p256_csrs_are_rejected() -> Result<(), TestError> {
        let params = CertificateParams::default();
        let p256_key = KeyPair::generate().map_err(|_| TestError::Setup)?;
        let valid = csr(&params, &p256_key)?;

        let mut trailing = valid.clone();
        trailing.push(0);
        let mut tampered = valid;
        let last = tampered.last_mut().ok_or(TestError::Setup)?;
        *last ^= 1;
        let p384_key =
            KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).map_err(|_| TestError::Setup)?;
        let p384 = csr(&params, &p384_key)?;

        for invalid in [
            &[][..],
            trailing.as_slice(),
            tampered.as_slice(),
            p384.as_slice(),
        ] {
            let error = GatewayIssuer::validate_csr(invalid)
                .err()
                .ok_or(TestError::Assertion)?;
            if !matches!(error, GatewayIssuerError::InvalidCsr) {
                return Err(TestError::Assertion);
            }
        }
        Ok(())
    }

    #[test]
    fn authority_key_and_packaged_root_must_match() -> Result<(), TestError> {
        let ca = TestCa::generate()?;
        let other = TestCa::generate()?;
        let key_error = GatewayIssuer::new(
            &ca.certificate_der,
            &PrivatePkcs8KeyDer::from(other.private_key_der.as_slice()),
            &ca.certificate_der,
            &site()?,
        )
        .err()
        .ok_or(TestError::Assertion)?;
        if key_error != GatewayIssuerError::OriginCa {
            return Err(TestError::Assertion);
        }

        let root_error = GatewayIssuer::new(
            &ca.certificate_der,
            &PrivatePkcs8KeyDer::from(ca.private_key_der.as_slice()),
            &other.certificate_der,
            &site()?,
        )
        .err()
        .ok_or(TestError::Assertion)?;
        if !matches!(root_error, GatewayIssuerError::TrustRootMismatch) {
            return Err(TestError::Assertion);
        }
        Ok(())
    }

    #[test]
    fn load_accepts_a_single_pem_packaged_root() -> Result<(), TestError> {
        let ca = TestCa::generate()?;
        let directory = TestDirectory::new()?;
        let certificate_path = directory.write("origin-ca.der", ca.certificate_der.as_ref())?;
        let key_path = directory.write("origin-ca-key.pk8", &ca.private_key_der)?;
        let root_path =
            directory.write("local-origin-ca.crt", &pem(ca.certificate_der.as_ref()))?;

        GatewayIssuer::load(&certificate_path, &key_path, &root_path, &site()?)
            .map(|_| ())
            .map_err(|_| TestError::Assertion)
    }

    #[test]
    fn validation_does_not_increment_the_issue_counter() -> Result<(), TestError> {
        let (issuer, issue_count) = GatewayIssuer::for_test().map_err(|_| TestError::Setup)?;
        let params = CertificateParams::default();
        let key = KeyPair::generate().map_err(|_| TestError::Setup)?;
        let csr_der = csr(&params, &key)?;

        GatewayIssuer::validate_csr(&csr_der).map_err(|_| TestError::Assertion)?;
        if issue_count.load(Ordering::Relaxed) != 0 {
            return Err(TestError::Assertion);
        }
        issuer
            .issue(Uuid::now_v7(), &csr_der)
            .map_err(|_| TestError::Assertion)?;
        if issue_count.load(Ordering::Relaxed) != 1 {
            return Err(TestError::Assertion);
        }
        Ok(())
    }

    fn ca_certificate(
        ca: &TestCa,
    ) -> Result<x509_parser::certificate::X509Certificate<'_>, TestError> {
        let (remainder, certificate) =
            parse_x509_certificate(ca.certificate_der.as_ref()).map_err(|_| TestError::Setup)?;
        if !remainder.is_empty() {
            return Err(TestError::Setup);
        }
        Ok(certificate)
    }

    fn pem(der: &[u8]) -> Vec<u8> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(der);
        format!("-----BEGIN CERTIFICATE-----\n{encoded}\n-----END CERTIFICATE-----\n").into_bytes()
    }
}
