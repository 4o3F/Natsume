use std::{fs, path::Path, sync::Arc, time::SystemTime};

pub(super) use rcgen::CertificateSigningRequestParams;
use rcgen::{
    CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PublicKeyData, SerialNumber, SubjectPublicKeyInfo,
};
use rustls::{
    RootCertStore,
    client::{WebPkiServerVerifier, danger::ServerCertVerifier as _},
    server::ParsedCertificate,
};
pub(super) use rustls_pki_types::CertificateSigningRequestDer;
use rustls_pki_types::{
    CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime, pem::PemObject as _,
};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use time::OffsetDateTime;
use x509_parser::{certification_request::X509CertificationRequest, prelude::FromDer as _};
use zeroize::Zeroize as _;

use crate::{config::GatewaySiteConfig, tls};

pub(crate) const GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS: i64 = 300;
/// The contest network is offline and has no NTP, so a device RTC behind the server must not
/// fail-close fresh-leaf finalization with webpki `NotValidYet`. One hour covers realistic RTC
/// drift; a clock that is years behind (dead RTC battery) still fails closed and stays a
/// registered residual in the Phase 3 ledger.
pub(crate) const GATEWAY_NOT_BEFORE_BACKDATE_SECONDS: i64 = 3600;

pub(super) const CERTIFICATE_SERIAL_BYTES: usize = 20;
#[cfg(test)]
pub(crate) const TEST_GATEWAY_HOSTNAME: &str = "gateway.contest.example";
#[cfg(test)]
pub(crate) const TEST_GATEWAY_NOT_AFTER: &str = "4090-01-01T00:00:00Z";
#[cfg(test)]
pub(crate) const TEST_CONTEST_END: &str = "4089-12-31T00:00:00Z";

#[derive(Clone)]
pub(crate) struct GatewayIssuer(pub(super) Arc<GatewayIssuerInner>);

pub(super) struct GatewayIssuerInner {
    pub(super) issuer: Issuer<'static, KeyPair>,
    pub(super) origin_certificate_der: Vec<u8>,
    pub(super) site: GatewaySiteConfig,
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

    pub(super) fn verify_issuing_material(&self) -> Result<(), GatewayIssuerError> {
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

    pub(super) fn sign_public_key(
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
        let not_before = now
            .checked_sub(GATEWAY_NOT_BEFORE_BACKDATE_SECONDS)
            .ok_or(GatewayIssuerError::ClockInvalid)?;
        params.not_before = OffsetDateTime::from_unix_timestamp(not_before)
            .map_err(|_| GatewayIssuerError::ClockInvalid)?;
        params.not_after = self.0.site.gateway_not_after().timestamp();
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
            .ok_or(GatewayIssuerError::ClockInvalid)?;
        let contest_end = not_after
            .checked_sub(crate::config::GATEWAY_VALIDITY_MARGIN_SECONDS)
            .ok_or(GatewayIssuerError::ClockInvalid)?;
        let encoded_not_after = encode_utc_timestamp(not_after)?;
        let encoded_contest_end = encode_utc_timestamp(contest_end)?;
        let site = GatewaySiteConfig::for_test(
            TEST_GATEWAY_HOSTNAME,
            &encoded_not_after,
            &encoded_contest_end,
        )
        .map_err(|_| GatewayIssuerError::InvalidMaterial)?;
        Self::for_test_with_site(site)
    }

    #[cfg(test)]
    pub(super) fn for_test_with_site(site: GatewaySiteConfig) -> Result<Self, GatewayIssuerError> {
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

pub(super) fn read_packaged_origin_root(
    path: &Path,
) -> Result<CertificateDer<'static>, GatewayIssuerError> {
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

pub(super) fn raw_csr_spki_der(csr_der: &[u8]) -> Result<&[u8], GatewayIssuerError> {
    let (remainder, csr) =
        X509CertificationRequest::from_der(csr_der).map_err(|_| GatewayIssuerError::InvalidCsr)?;
    if !remainder.is_empty() {
        return Err(GatewayIssuerError::InvalidCsr);
    }
    Ok(csr.certification_request_info.subject_pki.raw)
}

pub(super) fn raw_csr_spki_sha256(csr_der: &[u8]) -> Result<[u8; 32], GatewayIssuerError> {
    Ok(Sha256::digest(raw_csr_spki_der(csr_der)?).into())
}

#[cfg(test)]
pub(super) fn encode_utc_timestamp(unix_seconds: i64) -> Result<String, GatewayIssuerError> {
    OffsetDateTime::from_unix_timestamp(unix_seconds)
        .map_err(|_| GatewayIssuerError::ClockInvalid)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| GatewayIssuerError::ClockInvalid)
}

pub(super) fn current_unix_seconds() -> Result<i64, GatewayIssuerError> {
    let elapsed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| GatewayIssuerError::ClockInvalid)?;
    i64::try_from(elapsed.as_secs()).map_err(|_| GatewayIssuerError::ClockInvalid)
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

#[cfg(test)]
mod tests {
    use rcgen::KeyPair;
    use x509_parser::{certificate::X509Certificate, prelude::FromDer as _};

    use super::{GATEWAY_NOT_BEFORE_BACKDATE_SECONDS, GatewayIssuer, current_unix_seconds};

    #[test]
    fn gateway_leaf_not_before_is_backdated_one_hour() {
        let before = match current_unix_seconds() {
            Ok(before) => before,
            Err(error) => panic!("current time must be available: {error}"),
        };
        let signer = match GatewayIssuer::for_test() {
            Ok(signer) => signer,
            Err(error) => panic!("test issuer must be created: {error}"),
        };
        let key = match KeyPair::generate() {
            Ok(key) => key,
            Err(error) => panic!("test key must be generated: {error}"),
        };
        let certificate = match signer.sign_public_key(&key) {
            Ok(certificate) => certificate,
            Err(error) => panic!("Gateway leaf must be issued: {error}"),
        };
        let (remainder, leaf) = match X509Certificate::from_der(&certificate.leaf_der) {
            Ok(parsed) => parsed,
            Err(error) => panic!("issued Gateway leaf must parse: {error}"),
        };
        assert!(
            remainder.is_empty(),
            "issued Gateway leaf must be exact DER"
        );
        let not_before_unix = leaf.validity().not_before.timestamp();
        let after = match current_unix_seconds() {
            Ok(after) => after,
            Err(error) => panic!("current time must be available: {error}"),
        };

        assert!(
            before - GATEWAY_NOT_BEFORE_BACKDATE_SECONDS <= not_before_unix
                && not_before_unix <= after - GATEWAY_NOT_BEFORE_BACKDATE_SECONDS
        );
    }
}
