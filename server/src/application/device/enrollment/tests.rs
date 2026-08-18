use std::{fs, net::Ipv4Addr};

use rcgen::{CertificateParams, KeyPair};
use sha2::{Digest, Sha256};
use snafu::Snafu;
use x509_parser::{certification_request::X509CertificationRequest, prelude::FromDer as _};

use crate::{
    config::{GatewaySiteConfig, ORIGIN_CA_CERTIFICATE_FILENAME, ORIGIN_CA_PRIVATE_KEY_FILENAME},
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
    let (remainder, parsed) =
        X509CertificationRequest::from_der(csr.der()).map_err(|_| TestFailure::FixtureFailed)?;
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

    let other = TestIdentity::new(Ipv4Addr::LOCALHOST).map_err(|_| TestFailure::FixtureFailed)?;
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
