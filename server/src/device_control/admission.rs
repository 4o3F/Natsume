use natsume_device_protocol::{
    generated::{
        ClientHandshakeEnvelope, ClientProof, EnrollmentAuthority, EnrollmentEvidenceQuality,
        ServerChallenge, client_handshake_envelope, client_proof,
    },
    verify_client_proof,
};
use semver::Version;
use snafu::Snafu;

use crate::component::device::{
    ControlAuthority, ControlPublicKey, EvidenceQuality, MachineHardwareId,
    ValidatedEnrollmentEvidence,
};

/// The connection-local window in which exactly one Client proof may be submitted.
///
/// Taking the challenge during submission makes every attempt terminal, including a
/// malformed one. The caller closes the connection on any [`AdmissionError`] rather
/// than reopening the proof window.
pub(super) struct ProofWindow {
    challenge: Option<ServerChallenge>,
}

impl ProofWindow {
    pub(super) fn new() -> Result<Self, AdmissionError> {
        let mut challenge_nonce = [0_u8; 32];
        getrandom::fill(&mut challenge_nonce).map_err(|_| AdmissionError::EntropyUnavailable)?;
        Ok(Self::from_challenge_nonce(challenge_nonce))
    }

    fn from_challenge_nonce(challenge_nonce: [u8; 32]) -> Self {
        Self {
            challenge: Some(ServerChallenge {
                challenge_nonce: challenge_nonce.to_vec(),
            }),
        }
    }

    pub(super) fn server_challenge(&self) -> Option<&ServerChallenge> {
        self.challenge.as_ref()
    }

    pub(super) fn submit(
        &mut self,
        envelope: ClientHandshakeEnvelope,
    ) -> Result<ProofSubmission, AdmissionError> {
        let challenge = self
            .challenge
            .take()
            .ok_or(AdmissionError::ProofWindowConsumed)?;
        let Some(client_handshake_envelope::Body::ClientProof(proof)) = envelope.body else {
            return Err(AdmissionError::UnexpectedHandshakeMessage);
        };
        classify_proof(challenge, proof)
    }
}

/// Semantically classified proof ready for its purpose-specific authority step.
///
/// Enrollment has already verified the candidate key, while Resume deliberately
/// retains the signed proof until the Device Component selects the current key.
pub(super) enum ProofSubmission {
    /// A proved candidate awaiting replay classification or manual review.
    Enrollment(EnrollmentPreAuth),
    /// A validated Machine Hardware ID whose proof awaits current-authority verification.
    Resume(ResumeProof),
}

/// Connection-local proof that an Enrollment candidate passed semantic validation and
/// demonstrated possession of its candidate control key.
///
/// This is deliberately distinct from [`ValidatedEnrollmentEvidence`]: a clone of the
/// evidence may enter the Device Component's pending-review registry, while this value
/// remains on the originating connection. Consuming it in [`Self::activated`] binds the
/// eventual authority to the exact candidate key proved by that connection before an
/// [`EnrollmentReadyBarrier`] can be created.
pub(super) struct EnrollmentPreAuth {
    evidence: ValidatedEnrollmentEvidence,
}

impl EnrollmentPreAuth {
    /// Copies the non-secret facts needed for manual review without discarding the
    /// connection's proof-to-activation binding.
    pub(super) fn review_evidence(&self) -> ValidatedEnrollmentEvidence {
        self.evidence.clone()
    }

    /// Accepts only an authority for the candidate key proved by this connection.
    pub(super) fn activated(
        self,
        authority: ControlAuthority,
    ) -> Result<EnrollmentReadyBarrier, AdmissionError> {
        if authority.control_public_key() != self.evidence.candidate_public_key() {
            return Err(AdmissionError::EnrollmentActivationMismatch);
        }
        let device_id = authority.device_id();
        Ok(EnrollmentReadyBarrier {
            expected_authority: EnrollmentAuthority {
                device_id: device_id.as_text(),
            },
            authority,
        })
    }
}

/// A semantic Resume proof awaiting the Device Component's authority selection.
///
/// Keeping the original challenge and proof here prevents the transport from
/// authenticating against a caller-selected key. The value is consumed by
/// [`Self::verify_authority`], so it cannot authorize more than one admission path.
pub(super) struct ResumeProof {
    machine_hardware_id: MachineHardwareId,
    challenge: ServerChallenge,
    proof: ClientProof,
}

impl ResumeProof {
    pub(super) const fn machine_hardware_id(&self) -> MachineHardwareId {
        self.machine_hardware_id
    }

    pub(super) fn verify_authority(
        self,
        authority: Option<ControlAuthority>,
    ) -> Result<ControlAuthority, AdmissionError> {
        let authority = authority.ok_or(AdmissionError::ResumeAuthorityUnavailable)?;
        verify_client_proof(
            authority.control_public_key().as_bytes(),
            &self.challenge,
            &self.proof,
        )
        .map_err(|_| AdmissionError::ProofRejected)?;
        if !authority.device_state().is_enabled() {
            return Err(AdmissionError::ResumeAuthorityInactive);
        }
        Ok(authority)
    }
}

/// The exact activated authority echo required before a Device may proceed.
///
/// This connection-local barrier is created only after candidate-key activation
/// matches [`EnrollmentPreAuth`]. It releases the same enabled authority only when
/// the Client echoes its Device ID; mismatch or inactivity terminates admission.
pub(super) struct EnrollmentReadyBarrier {
    expected_authority: EnrollmentAuthority,
    authority: ControlAuthority,
}

impl EnrollmentReadyBarrier {
    pub(super) const fn enrollment_activated(&self) -> &EnrollmentAuthority {
        &self.expected_authority
    }

    /// Validates the exact authority echo. Repeating the same echo is idempotent.
    pub(super) fn receive(
        &self,
        envelope: ClientHandshakeEnvelope,
    ) -> Result<ControlAuthority, AdmissionError> {
        match envelope.body {
            Some(client_handshake_envelope::Body::EnrollmentReady(authority))
                if authority == self.expected_authority =>
            {
                if self.authority.device_state().is_enabled() {
                    Ok(self.authority)
                } else {
                    Err(AdmissionError::EnrollmentAuthorityInactive)
                }
            }
            Some(client_handshake_envelope::Body::EnrollmentReady(_)) => {
                Err(AdmissionError::EnrollmentAuthorityMismatch)
            }
            _ => Err(AdmissionError::UnexpectedHandshakeMessage),
        }
    }
}

/// Validates shared proof metadata and separates Enrollment verification from
/// Resume's database-selected-key verification.
fn classify_proof(
    challenge: ServerChallenge,
    proof: ClientProof,
) -> Result<ProofSubmission, AdmissionError> {
    let machine_hardware_id = validate_metadata(&proof)?;
    match proof
        .purpose
        .clone()
        .ok_or(AdmissionError::MissingProofPurpose)?
    {
        client_proof::Purpose::Enrollment(attempt) => {
            let candidate_public_key = ControlPublicKey::parse(&attempt.candidate_public_key)
                .ok_or(AdmissionError::InvalidCandidatePublicKey)?;
            let evidence_quality =
                match EnrollmentEvidenceQuality::try_from(attempt.evidence_quality) {
                    Ok(EnrollmentEvidenceQuality::Medium) => EvidenceQuality::Medium,
                    Ok(EnrollmentEvidenceQuality::Strong) => EvidenceQuality::Strong,
                    Ok(EnrollmentEvidenceQuality::Unspecified) | Err(_) => {
                        return Err(AdmissionError::InvalidEnrollmentEvidenceQuality);
                    }
                };
            verify_client_proof(candidate_public_key.as_bytes(), &challenge, &proof)
                .map_err(|_| AdmissionError::ProofRejected)?;
            Ok(ProofSubmission::Enrollment(EnrollmentPreAuth {
                evidence: ValidatedEnrollmentEvidence::new(
                    machine_hardware_id,
                    candidate_public_key,
                    evidence_quality,
                    proof.daemon_version,
                    proof.agent_version,
                ),
            }))
        }
        client_proof::Purpose::Resume(_) => Ok(ProofSubmission::Resume(ResumeProof {
            machine_hardware_id,
            challenge,
            proof,
        })),
    }
}

fn validate_metadata(proof: &ClientProof) -> Result<MachineHardwareId, AdmissionError> {
    if !is_canonical_version(&proof.daemon_version) {
        return Err(AdmissionError::InvalidDaemonVersion);
    }
    if !is_canonical_version(&proof.agent_version) {
        return Err(AdmissionError::InvalidAgentVersion);
    }
    MachineHardwareId::parse(&proof.machine_hardware_id)
        .ok_or(AdmissionError::InvalidMachineHardwareId)
}

fn is_canonical_version(value: &str) -> bool {
    Version::parse(value).is_ok_and(|parsed| parsed.to_string() == value)
}

/// Errors that terminate the current Device Control admission attempt.
///
/// The transport does not retry or downgrade any of these failures on the same
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub(super) enum AdmissionError {
    #[snafu(display("the connection proof window was already consumed"))]
    ProofWindowConsumed,
    #[snafu(display("unexpected Device Control handshake message"))]
    UnexpectedHandshakeMessage,
    #[snafu(display("challenge entropy is unavailable"))]
    EntropyUnavailable,
    #[snafu(display("the Daemon version is not canonical SemVer"))]
    InvalidDaemonVersion,
    #[snafu(display("the Session Agent version is not canonical SemVer"))]
    InvalidAgentVersion,
    #[snafu(display("the Machine Hardware ID is invalid"))]
    InvalidMachineHardwareId,
    #[snafu(display("the Client proof purpose is missing"))]
    MissingProofPurpose,
    #[snafu(display("the candidate control public key is invalid"))]
    InvalidCandidatePublicKey,
    #[snafu(display("the Enrollment evidence quality is invalid"))]
    InvalidEnrollmentEvidenceQuality,
    #[snafu(display("the Client proof was rejected"))]
    ProofRejected,
    #[snafu(display("no current Resume authority is available"))]
    ResumeAuthorityUnavailable,
    #[snafu(display("the current Resume authority is not enabled"))]
    ResumeAuthorityInactive,
    #[snafu(display("the Enrollment authority echo does not match"))]
    EnrollmentAuthorityMismatch,
    #[snafu(display("the activated Enrollment authority is not enabled"))]
    EnrollmentAuthorityInactive,
    #[snafu(display("the committed Enrollment activation does not match the proved candidate"))]
    EnrollmentActivationMismatch,
}

#[cfg(test)]
mod tests;
