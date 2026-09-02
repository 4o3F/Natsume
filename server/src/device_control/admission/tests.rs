use ed25519_dalek::SigningKey;
use natsume_device_protocol::{
    generated::{
        ClientHandshakeEnvelope, ClientProof, EnrollmentAttempt, EnrollmentAuthority,
        EnrollmentEvidenceQuality, ResumeSession, client_handshake_envelope, client_proof,
    },
    sign_client_proof,
};

use crate::component::device::{
    ControlAuthority, ControlPublicKey, DeviceId, DeviceState, EvidenceQuality,
};

use super::{AdmissionError, ProofSubmission, ProofWindow, is_canonical_version};

const MACHINE_HARDWARE_ID: &str = "a9aa9d04-3ece-5567-8260-910930ff5e03";
const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000001";
const CHALLENGE_NONCE: [u8; 32] = [0xA5; 32];

fn proof_window() -> ProofWindow {
    ProofWindow::from_challenge_nonce(CHALLENGE_NONCE)
}

fn unsigned_proof(purpose: client_proof::Purpose) -> ClientProof {
    ClientProof {
        daemon_version: "2.0.0".to_owned(),
        agent_version: "2.0.0-rc.1+packaging.7".to_owned(),
        machine_hardware_id: MACHINE_HARDWARE_ID.to_owned(),
        signature: Vec::new(),
        purpose: Some(purpose),
    }
}

fn signed_proof(signing_key: &SigningKey, purpose: client_proof::Purpose) -> ClientProof {
    let window = proof_window();
    sign_client_proof(
        signing_key,
        window
            .server_challenge()
            .unwrap_or_else(|| panic!("a fresh proof window had no challenge")),
        unsigned_proof(purpose),
    )
}

fn proof_envelope(proof: ClientProof) -> ClientHandshakeEnvelope {
    ClientHandshakeEnvelope {
        body: Some(client_handshake_envelope::Body::ClientProof(proof)),
    }
}

fn resume_proof(signing_key: &SigningKey) -> ClientProof {
    signed_proof(signing_key, client_proof::Purpose::Resume(ResumeSession {}))
}

fn enrollment_proof(signing_key: &SigningKey) -> ClientProof {
    signed_proof(
        signing_key,
        client_proof::Purpose::Enrollment(EnrollmentAttempt {
            candidate_public_key: signing_key.verifying_key().to_bytes().to_vec(),
            evidence_quality: EnrollmentEvidenceQuality::Strong.into(),
        }),
    )
}

#[test]
fn canonical_versions_reject_alternate_or_incomplete_spellings() {
    for valid in ["0.0.0", "2.0.0", "2.0.0-rc.1+packaging.7"] {
        assert!(is_canonical_version(valid));
    }
    for invalid in ["", "1.2", "01.2.3", "v1.2.3", "1.2.3+build..7"] {
        assert!(!is_canonical_version(invalid));
    }
}

#[test]
fn fresh_proof_windows_own_distinct_exact_length_challenges() {
    let first = ProofWindow::new()
        .unwrap_or_else(|error| panic!("first challenge generation failed: {error}"));
    let second = ProofWindow::new()
        .unwrap_or_else(|error| panic!("second challenge generation failed: {error}"));
    let first_nonce = &first
        .server_challenge()
        .unwrap_or_else(|| panic!("first proof window had no challenge"))
        .challenge_nonce;
    let second_nonce = &second
        .server_challenge()
        .unwrap_or_else(|| panic!("second proof window had no challenge"))
        .challenge_nonce;
    assert_eq!(first_nonce.len(), 32);
    assert_eq!(second_nonce.len(), 32);
    assert_ne!(first_nonce, second_nonce);
}

#[test]
fn first_submission_consumes_the_unique_proof_window() {
    let signing_key = SigningKey::from_bytes(&[0x11; 32]);
    let proof = resume_proof(&signing_key);
    let mut window = proof_window();
    assert!(matches!(
        window.submit(proof_envelope(proof.clone())),
        Ok(ProofSubmission::Resume(_))
    ));
    assert!(matches!(
        window.submit(proof_envelope(proof)),
        Err(AdmissionError::ProofWindowConsumed)
    ));
}

#[test]
fn malformed_first_submission_also_consumes_the_proof_window() {
    let mut window = proof_window();
    assert!(matches!(
        window.submit(ClientHandshakeEnvelope { body: None }),
        Err(AdmissionError::UnexpectedHandshakeMessage)
    ));
    assert!(matches!(
        window.submit(ClientHandshakeEnvelope { body: None }),
        Err(AdmissionError::ProofWindowConsumed)
    ));
}

#[test]
fn enrollment_verifies_the_candidate_key_and_builds_review_evidence() {
    let candidate_key = SigningKey::from_bytes(&[0x22; 32]);
    let mut window = proof_window();
    let Ok(ProofSubmission::Enrollment(enrollment)) =
        window.submit(proof_envelope(enrollment_proof(&candidate_key)))
    else {
        panic!("valid Enrollment proof was rejected");
    };
    let evidence = enrollment.review_evidence();
    assert_eq!(
        evidence.machine_hardware_id().as_text(),
        MACHINE_HARDWARE_ID
    );
    assert_eq!(
        evidence.candidate_public_key().as_bytes(),
        &candidate_key.verifying_key().to_bytes()
    );
    assert_eq!(evidence.evidence_quality(), EvidenceQuality::Strong);
    assert_eq!(evidence.daemon_version(), "2.0.0");
    assert_eq!(evidence.agent_version(), "2.0.0-rc.1+packaging.7");

    let wrong_key = SigningKey::from_bytes(&[0x23; 32]);
    let mut invalid = enrollment_proof(&candidate_key);
    let Some(client_proof::Purpose::Enrollment(attempt)) = invalid.purpose.as_mut() else {
        panic!("Enrollment fixture lost its purpose");
    };
    attempt.candidate_public_key = wrong_key.verifying_key().to_bytes().to_vec();
    let mut window = proof_window();
    assert!(matches!(
        window.submit(proof_envelope(invalid)),
        Err(AdmissionError::ProofRejected)
    ));
}

#[test]
fn proof_semantics_reject_invalid_identity_purpose_and_quality() {
    let signing_key = SigningKey::from_bytes(&[0x31; 32]);

    let mut missing = unsigned_proof(client_proof::Purpose::Resume(ResumeSession {}));
    missing.purpose = None;
    let mut window = proof_window();
    assert!(matches!(
        window.submit(proof_envelope(missing)),
        Err(AdmissionError::MissingProofPurpose)
    ));

    let mut invalid_machine = resume_proof(&signing_key);
    invalid_machine.machine_hardware_id = DEVICE_ID.to_owned();
    let mut window = proof_window();
    assert!(matches!(
        window.submit(proof_envelope(invalid_machine)),
        Err(AdmissionError::InvalidMachineHardwareId)
    ));

    let mut invalid_quality = enrollment_proof(&signing_key);
    let Some(client_proof::Purpose::Enrollment(attempt)) = invalid_quality.purpose.as_mut() else {
        panic!("Enrollment fixture lost its purpose");
    };
    attempt.evidence_quality = EnrollmentEvidenceQuality::Unspecified.into();
    let mut window = proof_window();
    assert!(matches!(
        window.submit(proof_envelope(invalid_quality)),
        Err(AdmissionError::InvalidEnrollmentEvidenceQuality)
    ));
}

#[test]
fn resume_verifies_the_selected_current_enabled_authority() {
    let signing_key = SigningKey::from_bytes(&[0x41; 32]);
    let device_id =
        DeviceId::parse(DEVICE_ID).unwrap_or_else(|| panic!("Device fixture ID was invalid"));
    let control_public_key = ControlPublicKey::parse(&signing_key.verifying_key().to_bytes())
        .unwrap_or_else(|| panic!("control-key fixture was invalid"));

    let mut window = proof_window();
    let Ok(ProofSubmission::Resume(resume)) =
        window.submit(proof_envelope(resume_proof(&signing_key)))
    else {
        panic!("Resume fixture was rejected before authority selection");
    };
    assert_eq!(resume.machine_hardware_id().as_text(), MACHINE_HARDWARE_ID);
    let authority = ControlAuthority::new(device_id, control_public_key, DeviceState::Enabled)
        .unwrap_or_else(|| panic!("enabled authority fixture was rejected"));
    assert_eq!(resume.verify_authority(Some(authority)), Ok(authority));

    let mut window = proof_window();
    let Ok(ProofSubmission::Resume(resume)) =
        window.submit(proof_envelope(resume_proof(&signing_key)))
    else {
        panic!("Resume fixture was rejected before authority selection");
    };
    assert!(matches!(
        resume.verify_authority(ControlAuthority::new(
            device_id,
            control_public_key,
            DeviceState::Disabled,
        )),
        Err(AdmissionError::ResumeAuthorityInactive)
    ));
}

#[test]
fn enrollment_ready_requires_the_exact_authority_echo() {
    let signing_key = SigningKey::from_bytes(&[0x51; 32]);
    let device_id =
        DeviceId::parse(DEVICE_ID).unwrap_or_else(|| panic!("Device fixture ID was invalid"));
    let mut window = proof_window();
    let Ok(ProofSubmission::Enrollment(enrollment)) =
        window.submit(proof_envelope(enrollment_proof(&signing_key)))
    else {
        panic!("Enrollment fixture was rejected");
    };
    let control_public_key = ControlPublicKey::parse(&signing_key.verifying_key().to_bytes())
        .unwrap_or_else(|| panic!("control-key fixture was invalid"));
    let authority = ControlAuthority::new(device_id, control_public_key, DeviceState::Enabled)
        .unwrap_or_else(|| panic!("enabled authority fixture was rejected"));
    let barrier = enrollment
        .activated(authority)
        .unwrap_or_else(|error| panic!("matching activation was rejected: {error}"));
    let ready = ClientHandshakeEnvelope {
        body: Some(client_handshake_envelope::Body::EnrollmentReady(
            barrier.enrollment_activated().clone(),
        )),
    };
    assert_eq!(barrier.receive(ready.clone()), Ok(authority));
    assert_eq!(barrier.receive(ready), Ok(authority));

    let mismatch = ClientHandshakeEnvelope {
        body: Some(client_handshake_envelope::Body::EnrollmentReady(
            EnrollmentAuthority {
                device_id: "01900000-0000-7000-8000-000000000002".to_owned(),
            },
        )),
    };
    assert_eq!(
        barrier.receive(mismatch),
        Err(AdmissionError::EnrollmentAuthorityMismatch)
    );
}
