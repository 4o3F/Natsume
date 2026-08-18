use std::net::IpAddr;

use base64::Engine as _;
use subtle::ConstantTimeEq as _;

use super::{
    ENROLLMENT_PROTOCOL_VERSION, EnrollmentError, EnrollmentRequestInput,
    MAX_GATEWAY_CSR_DER_BYTES, ValidatedEnrollmentRequest,
    identifier::parse_canonical_uuid,
    issuer::{CertificateSigningRequestDer, CertificateSigningRequestParams, raw_csr_spki_sha256},
};

const MAX_CLIENT_VERSION_BYTES: usize = 64;

pub(super) fn validate_request(
    input: EnrollmentRequestInput,
    source_ip: IpAddr,
) -> Result<ValidatedEnrollmentRequest, EnrollmentError> {
    parse_canonical_uuid(&input.machine_hardware_id, 5)
        .map_err(|()| EnrollmentError::InvalidMachineHardwareId)?;
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
        hardware_identity_quality: input.hardware_identity_quality,
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
