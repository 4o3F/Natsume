#[cfg(test)]
mod orchestration_tests {
    use std::{
        fs,
        net::{IpAddr, Ipv4Addr},
        path::PathBuf,
    };

    use diesel::{
        QueryableByName, RunQueryDsl,
        connection::SimpleConnection,
        sql_types::{BigInt, Binary, Text},
    };
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PublicKeyData};
    use rustls::{client::verify_server_name, server::ParsedCertificate};
    use rustls_pki_types::{CertificateDer, ServerName};
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::{
            device::{
                self, DeviceId, HardwareIdentityQuality,
                enrollment::{
                    self as enrollment, EnrollmentError, EnrollmentOutcome, EnrollmentRequestId,
                    EnrollmentRequestInput, GatewayIssuer, IntakeIds, MAX_LIVE_ENROLLMENT_REQUESTS,
                    TEST_CONTEST_END, TEST_GATEWAY_HOSTNAME, TEST_GATEWAY_NOT_AFTER,
                    ValidatedEnrollmentRequest, encode_standard_base64,
                },
            },
            provisioning,
        },
        audit::{AuditEventId, CorrelationId},
        config::GatewaySiteConfig,
        db::{Database, DatabaseConfig},
    };

    use super::super::intake_with_ids;
    use x509_parser::{extensions::GeneralName, parse_x509_certificate};

    const SOURCE_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 44));
    const HOSTILE_CSR_SERIAL_SUGGESTION: [u8; 20] = [0x5a; 20];
    const EXPECTED_SUBJECT_COMMON_NAMES: [&str; 0] = [];
    const EXPECTED_SERVER_AUTH_EKU_DER: &[u8] = &[
        0x30, 0x0a, 0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01,
    ];

    #[tokio::test]
    async fn closed_window_is_zero_write_and_create_issuance_is_secret_safe_and_site_authoritative()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        let request = RequestFixture::new("hostile.device.example")?;
        let before = business_counts(&database).await?;
        let result = enrollment::intake(
            &database,
            gateway_signer.clone(),
            request.input.clone(),
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if result.err() != Some(EnrollmentError::ProvisioningWindowClosed)
            || business_counts(&database).await? != before
        {
            return Err(TestFailure::ClosedWindowWrote);
        }

        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let issued = match enrollment::intake(
            &database,
            gateway_signer,
            request.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::IntakeFailed)?
        {
            EnrollmentOutcome::Issued(issued) => issued,
            EnrollmentOutcome::Pending(_) => return Err(TestFailure::UnexpectedOutcome),
        };
        let token = *issued.device_token.as_bytes();
        let leaf = issued.gateway_leaf_der.clone();
        let evidence = issuance_evidence(&database).await?;
        let expected_audit_detail = format!(
            "{{\"resolution\":\"create_device\",\"certificate_serial\":\"{}\",\"gateway_spki_sha256\":\"{}\",\"previous_device_state\":null,\"evicted_live_connection\":false}}",
            evidence.certificate_serial,
            hex::encode(request.spki)
        );
        if evidence.devices != 1
            || evidence.requests != 1
            || evidence.tokens != 1
            || evidence.certificates != 1
            || evidence.active_certificates != 1
            || evidence.token_hash != Sha256::digest(token).as_slice()
            || evidence.certificate_spki != request.spki
            || evidence.request_state != "issued"
            || evidence.resolution != "create_device"
            || evidence.audit_actor != "device:enrollment"
            || evidence.audit_action != "issue_device_credentials"
            || evidence.audit_reason != "first_enrollment"
            || evidence.audit_detail != expected_audit_detail
        {
            return Err(TestFailure::IssuanceEvidenceChanged);
        }
        let certificate_der = CertificateDer::from(leaf.clone());
        let parsed = ParsedCertificate::try_from(&certificate_der)
            .map_err(|_| TestFailure::CertificateInvalid)?;
        let expected_name = ServerName::try_from("gateway.contest.example")
            .map_err(|_| TestFailure::CertificateInvalid)?;
        let hostile_name = ServerName::try_from("hostile.device.example")
            .map_err(|_| TestFailure::CertificateInvalid)?;
        let leaf_spki: [u8; 32] = Sha256::digest(parsed.subject_public_key_info().as_ref()).into();
        let leaf_profile = assert_exact_gateway_leaf_profile(&leaf)?;
        if verify_server_name(&parsed, &expected_name).is_err()
            || verify_server_name(&parsed, &hostile_name).is_ok()
            || leaf_spki != request.spki
            || leaf_profile.serial == request.serial_suggestion
        {
            return Err(TestFailure::CsrAuthorityEscaped);
        }
        let database_bytes = fixture.database_bytes()?;
        if contains_bytes(&database_bytes, &token) || contains_bytes(&database_bytes, &leaf) {
            return Err(TestFailure::PlaintextPersisted);
        }
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn pending_poll_is_idempotent_conflict_is_zero_write_and_close_expires_approved_claim()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let original = RequestFixture::new("first.invalid.example")?;
        let issued = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                original.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let replacement = RequestFixture::new("replacement.invalid.example")?;
        let first_pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let counts_after_pending = business_counts(&database).await?;
        let expected_create_detail = format!(
            "{{\"resolution\":\"replace_device_credentials\",\"state\":\"pending\",\"gateway_spki_sha256\":\"{}\"}}",
            hex::encode(replacement.spki)
        );
        let second_pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        if first_pending != second_pending
            || business_counts(&database).await? != counts_after_pending
            || audit_shape(&database, "create_enrollment_request").await?
                != (
                    "device:enrollment".to_owned(),
                    "credential_replacement".to_owned(),
                    expected_create_detail,
                )
        {
            return Err(TestFailure::PendingReplayWrote);
        }
        let conflicting = RequestFixture::new("other.invalid.example")?;
        let before_conflict = complete_counts(&database).await?;
        let conflict = enrollment::intake(
            &database,
            gateway_signer.clone(),
            conflicting.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if conflict.err() != Some(EnrollmentError::DeviceIdentityConflict)
            || complete_counts(&database).await? != before_conflict
        {
            return Err(TestFailure::ConflictWrote);
        }

        let credential_counts_before = credential_counts(&database).await?;
        enrollment::approve_request(
            &database,
            &EnrollmentRequestId::for_test(first_pending),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::DecisionFailed)?;
        if credential_counts(&database).await? != credential_counts_before
            || request_state(&database, first_pending).await? != "approved"
            || audit_shape(&database, "approve_enrollment_request").await?
                != (
                    "operator:self".to_owned(),
                    "operator_requested".to_owned(),
                    "{}".to_owned(),
                )
        {
            return Err(TestFailure::ApprovalIssued);
        }
        provisioning::close_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        if request_state(&database, first_pending).await? != "expired"
            || credential_counts(&database).await? != credential_counts_before
            || expiry_audit(&database).await? != (1, "window_closed".to_owned())
        {
            return Err(TestFailure::CloseDidNotExpire);
        }
        let poll = enrollment::intake(
            &database,
            gateway_signer.clone(),
            replacement.input.clone(),
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if poll.err() != Some(EnrollmentError::ProvisioningWindowClosed)
            || credential_counts(&database).await? != credential_counts_before
            || issued.device_id.to_string().is_empty()
        {
            return Err(TestFailure::ClosedClaimIssued);
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_spki_retry_reissues_once() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let original = RequestFixture::new("ignored.original.example")?;
        let serial_suggestion = original.serial_suggestion;
        let first = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                original.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let first_token = *first.device_token.as_bytes();
        let second = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer,
                original.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let certificate_states = certificate_states(&database).await?;
        let first_serial = parsed_leaf_serial(&first.gateway_leaf_der)?;
        let second_serial = parsed_leaf_serial(&second.gateway_leaf_der)?;
        if first.device_id != second.device_id
            || first_token == *second.device_token.as_bytes()
            || first_serial == second_serial
            || first_serial == serial_suggestion
            || second_serial == serial_suggestion
            || certificate_states != (1, 1)
            || business_counts(&database).await?.requests != 2
            || latest_issuance_reason(&database).await? != "same_spki_retry"
        {
            return Err(TestFailure::SameSpkiRetryChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejected_hardware_blocks_same_and_rotated_spki_until_window_close()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let original = RequestFixture::new("ignored.original.example")?;
        expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                original.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let replacement = RequestFixture::new("ignored.rejected.example")?;
        let pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let credentials_before = credential_counts(&database).await?;
        enrollment::reject_request(
            &database,
            &EnrollmentRequestId::for_test(pending),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::DecisionFailed)?;
        let poll = enrollment::intake(
            &database,
            gateway_signer.clone(),
            replacement.input.clone(),
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if poll.err() != Some(EnrollmentError::RequestRejected)
            || credential_counts(&database).await? != credentials_before
            || request_state(&database, pending).await? != "rejected"
            || audit_shape(&database, "reject_enrollment_request").await?
                != (
                    "operator:self".to_owned(),
                    "operator_requested".to_owned(),
                    "{}".to_owned(),
                )
        {
            return Err(TestFailure::RejectedPollChanged);
        }
        let rotated = RequestFixture::new("rotated-after-rejection.invalid.example")?;
        let before_rotated = complete_counts(&database).await?;
        let rotated_poll = enrollment::intake(
            &database,
            gateway_signer.clone(),
            rotated.input.clone(),
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if rotated_poll.err() != Some(EnrollmentError::RequestRejected)
            || complete_counts(&database).await? != before_rotated
        {
            return Err(TestFailure::RejectedKeyRotationWrote);
        }
        provisioning::close_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        if request_state(&database, pending).await? != "expired" {
            return Err(TestFailure::RejectedWindowBlockDidNotClear);
        }
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        expect_pending(
            enrollment::intake(
                &database,
                gateway_signer,
                rotated.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn revoked_and_disabled_devices_require_approval_then_reactivate_on_claim()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let request = RequestFixture::new("ignored-lifecycle.invalid.example")?;
        let first = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                request.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let device_id = DeviceId::parse(&first.device_id.to_string())
            .ok_or(TestFailure::LifecycleMutationFailed)?;

        device::revoke_device(&database, &device_id, correlation_id())
            .await
            .map_err(|_| TestFailure::LifecycleMutationFailed)?;
        let revoked_credentials = credential_counts(&database).await?;
        let revoked_pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                request.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        if credential_counts(&database).await? != revoked_credentials
            || device_state(&database, first.device_id).await? != "revoked"
        {
            return Err(TestFailure::LifecycleReplacementAutoApproved);
        }
        enrollment::approve_request(
            &database,
            &EnrollmentRequestId::for_test(revoked_pending),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::DecisionFailed)?;
        if credential_counts(&database).await? != revoked_credentials {
            return Err(TestFailure::ApprovalIssued);
        }
        let after_revoke = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                request.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        assert_reactivation_issuance(
            &database,
            first.device_id,
            after_revoke.device_id,
            "revoked",
        )
        .await?;

        device::disable_device(&database, &device_id, correlation_id())
            .await
            .map_err(|_| TestFailure::LifecycleMutationFailed)?;
        let disabled_credentials = credential_counts(&database).await?;
        let disabled_pending = expect_pending(
            enrollment::intake(
                &database,
                gateway_signer.clone(),
                request.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        if credential_counts(&database).await? != disabled_credentials
            || device_state(&database, first.device_id).await? != "disabled"
        {
            return Err(TestFailure::LifecycleReplacementAutoApproved);
        }
        enrollment::approve_request(
            &database,
            &EnrollmentRequestId::for_test(disabled_pending),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::DecisionFailed)?;
        if credential_counts(&database).await? != disabled_credentials {
            return Err(TestFailure::ApprovalIssued);
        }
        let after_disable = expect_issued(
            enrollment::intake(
                &database,
                gateway_signer,
                request.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        assert_reactivation_issuance(
            &database,
            first.device_id,
            after_disable.device_id,
            "disabled",
        )
        .await?;
        if active_certificate_count_for_device(&database, first.device_id).await? != 1 {
            return Err(TestFailure::ActiveCertificateInvariantChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn live_request_capacity_rejects_the_next_intake_without_writes()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        seed_live_request_capacity(&database).await?;
        let before = complete_counts(&database).await?;
        let request = RequestFixture::new("capacity.invalid.example")?;
        let result = enrollment::intake(
            &database,
            GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
            request.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if result.err() != Some(EnrollmentError::LiveRequestCapacityExceeded)
            || complete_counts(&database).await? != before
            || live_request_count(&database).await? != MAX_LIVE_ENROLLMENT_REQUESTS
        {
            return Err(TestFailure::LiveRequestCapacityWrote);
        }
        Ok(())
    }

    #[tokio::test]
    async fn csr_spki_mismatch_and_duplicate_issuance_audit_leave_zero_partial_state()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let gateway_signer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        let mut mismatch = RequestFixture::new("mismatch.invalid.example")?;
        mismatch.input.gateway_spki_sha256 = "00".repeat(32);
        let before = complete_counts(&database).await?;
        let mismatch_result = enrollment::intake(
            &database,
            gateway_signer.clone(),
            mismatch.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if mismatch_result.err() != Some(EnrollmentError::SpkiMismatch)
            || complete_counts(&database).await? != before
        {
            return Err(TestFailure::MismatchWrote);
        }

        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let request = RequestFixture::new("rollback.invalid.example")?;
        let duplicate_id = Uuid::now_v7();
        reserve_audit_id(&database, duplicate_id).await?;
        let before_duplicate = complete_counts(&database).await?;
        let result = intake_with_ids(
            &database,
            gateway_signer,
            request.validated(),
            correlation_id(),
            IntakeIds {
                request: Uuid::now_v7(),
                device: Uuid::now_v7(),
                certificate: Uuid::now_v7(),
                audit: AuditEventId::from_uuid(duplicate_id),
            },
        )
        .await;
        if result.err() != Some(EnrollmentError::PersistenceFailed)
            || complete_counts(&database).await? != before_duplicate
        {
            return Err(TestFailure::DuplicateAuditDidNotRollBack);
        }
        Ok(())
    }

    #[tokio::test]
    async fn injected_audit_token_and_certificate_failures_roll_back_exact_business_snapshots()
    -> Result<(), TestFailure> {
        for failure in [
            InjectedPersistenceFailure::Audit,
            InjectedPersistenceFailure::DeviceToken,
            InjectedPersistenceFailure::GatewayCertificate,
        ] {
            let fixture = DatabaseFixture::new();
            let database = fixture.connect().await?;
            provisioning::open_window(&database, correlation_id())
                .await
                .map_err(|_| TestFailure::WindowMutationFailed)?;
            install_injected_persistence_failure(&database, failure).await?;
            let before = rollback_snapshot(&database).await?;
            let request = RequestFixture::new("injected-rollback.invalid.example")?;
            let result = enrollment::intake(
                &database,
                GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?,
                request.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await;
            if result.err() != Some(EnrollmentError::PersistenceFailed)
                || rollback_snapshot(&database).await? != before
            {
                return Err(TestFailure::InjectedFailureDidNotRollBack);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn approved_claim_cas_failure_rolls_back_audit_and_preserves_all_credentials()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let issuer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let original = RequestFixture::new("claim-cas-original.invalid.example")?;
        expect_issued(
            enrollment::intake(
                &database,
                issuer.clone(),
                original.input,
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        let replacement = RequestFixture::new("claim-cas-replacement.invalid.example")?;
        let pending_id = expect_pending(
            enrollment::intake(
                &database,
                issuer.clone(),
                replacement.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        enrollment::approve_request(
            &database,
            &EnrollmentRequestId::for_test(pending_id),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::DecisionFailed)?;
        install_claim_cas_ignore(&database).await?;
        let before = rollback_snapshot(&database).await?;
        let result = enrollment::intake(
            &database,
            issuer,
            replacement.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if result.err() != Some(EnrollmentError::PersistenceFailed)
            || request_state(&database, pending_id).await? != "approved"
            || rollback_snapshot(&database).await? != before
        {
            return Err(TestFailure::ClaimCasFailureDidNotRollBack);
        }
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_current_credential_projection_rejects_without_writes()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = fixture.connect().await?;
        let issuer = GatewayIssuer::for_test().map_err(|_| TestFailure::IssuerFailed)?;
        provisioning::open_window(&database, correlation_id())
            .await
            .map_err(|_| TestFailure::WindowMutationFailed)?;
        let request = RequestFixture::new("credential-corruption.invalid.example")?;
        expect_issued(
            enrollment::intake(
                &database,
                issuer.clone(),
                request.input.clone(),
                SOURCE_IP,
                correlation_id(),
            )
            .await,
        )?;
        database
            .test_write(|connection| {
                diesel::sql_query(
                    "UPDATE gateway_certificates SET spki_sha256 = \
                 x'a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5' \
                 WHERE status = 'active'",
                )
                .execute(connection)
                .map(|_| ())
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)??;
        let before = rollback_snapshot(&database).await?;
        let result = enrollment::intake(
            &database,
            issuer,
            request.input,
            SOURCE_IP,
            correlation_id(),
        )
        .await;
        if result.err() != Some(EnrollmentError::InvalidPersistedFacts)
            || rollback_snapshot(&database).await? != before
        {
            return Err(TestFailure::CredentialCorruptionWrote);
        }
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum InjectedPersistenceFailure {
        Audit,
        DeviceToken,
        GatewayCertificate,
    }

    struct RequestFixture {
        input: EnrollmentRequestInput,
        csr_der: Vec<u8>,
        spki: [u8; 32],
        serial_suggestion: [u8; 20],
    }

    impl RequestFixture {
        fn new(hostile_san: &str) -> Result<Self, TestFailure> {
            let key = KeyPair::generate().map_err(|_| TestFailure::RequestFailed)?;
            let mut params = CertificateParams::new(vec![hostile_san.to_owned()])
                .map_err(|_| TestFailure::RequestFailed)?;
            let mut name = DistinguishedName::new();
            name.push(DnType::CommonName, "hostile-cn.invalid");
            name.push(
                DnType::CustomDnType(vec![2, 5, 4, 5]),
                hex::encode(HOSTILE_CSR_SERIAL_SUGGESTION),
            );
            params.distinguished_name = name;
            let csr = params
                .serialize_request(&key)
                .map_err(|_| TestFailure::RequestFailed)?;
            let csr_der = csr.der().to_vec();
            let spki: [u8; 32] = Sha256::digest(key.subject_public_key_info()).into();
            let machine_hardware_id =
                Uuid::new_v5(&Uuid::NAMESPACE_OID, b"natsume-enrollment-test-machine").to_string();
            Ok(Self {
                input: EnrollmentRequestInput {
                    machine_hardware_id,
                    hardware_identity_quality: HardwareIdentityQuality::Strong,
                    gateway_csr_der: encode_standard_base64(&csr_der),
                    gateway_spki_sha256: hex::encode(spki),
                    client_version: "2.0.0-test".to_owned(),
                    protocol_version: 1,
                },
                csr_der,
                spki,
                serial_suggestion: HOSTILE_CSR_SERIAL_SUGGESTION,
            })
        }

        fn validated(&self) -> ValidatedEnrollmentRequest {
            ValidatedEnrollmentRequest {
                machine_hardware_id: self.input.machine_hardware_id.clone(),
                hardware_identity_quality: HardwareIdentityQuality::Strong,
                gateway_csr_der: self.csr_der.clone(),
                gateway_spki_sha256: self.spki,
                client_version: self.input.client_version.clone(),
                protocol_version: self.input.protocol_version,
                source_ip: SOURCE_IP.to_string(),
            }
        }
    }

    struct ParsedLeafProfile {
        serial: Vec<u8>,
    }

    fn assert_exact_gateway_leaf_profile(
        leaf_der: &[u8],
    ) -> Result<ParsedLeafProfile, TestFailure> {
        let (remainder, certificate) =
            parse_x509_certificate(leaf_der).map_err(|_| TestFailure::CertificateInvalid)?;
        if !remainder.is_empty() {
            return Err(TestFailure::CertificateInvalid);
        }
        let subject_alt_name = certificate
            .subject_alternative_name()
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let exact_site_san = matches!(
            subject_alt_name.value.general_names.as_slice(),
            [GeneralName::DNSName(name)] if *name == TEST_GATEWAY_HOSTNAME
        );
        let subject_common_names = certificate
            .subject()
            .iter_common_name()
            .map(|name| name.as_str().map_err(|_| TestFailure::CertificateInvalid))
            .collect::<Result<Vec<_>, _>>()?;
        let extended_key_usage = certificate
            .extended_key_usage()
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let extended_key_usage_extension = certificate
            .get_extension_unique(&x509_parser::oid_registry::OID_X509_EXT_EXTENDED_KEY_USAGE)
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let exact_server_auth = extended_key_usage.value.server_auth
            && extended_key_usage_extension.value == EXPECTED_SERVER_AUTH_EKU_DER;
        let basic_constraints = certificate
            .basic_constraints()
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let exact_basic_constraints = basic_constraints.critical
            && !basic_constraints.value.ca
            && basic_constraints.value.path_len_constraint.is_none();
        let key_usage = certificate
            .key_usage()
            .map_err(|_| TestFailure::CertificateInvalid)?
            .ok_or(TestFailure::GatewayCertificateProfileChanged)?;
        let exact_key_usage = key_usage.critical && key_usage.value.flags == 1;
        let site = GatewaySiteConfig::for_test(
            TEST_GATEWAY_HOSTNAME,
            TEST_GATEWAY_NOT_AFTER,
            TEST_CONTEST_END,
        )
        .map_err(|_| TestFailure::CertificateInvalid)?;
        if !exact_site_san
            || subject_common_names.as_slice() != EXPECTED_SUBJECT_COMMON_NAMES
            || certificate.subject().iter_attributes().next().is_some()
            || !exact_server_auth
            || !exact_basic_constraints
            || !exact_key_usage
            || certificate.validity().not_after.timestamp()
                != site.gateway_not_after().unix_seconds()
        {
            return Err(TestFailure::GatewayCertificateProfileChanged);
        }
        let serial = certificate.raw_serial().to_vec();
        if serial.is_empty() || serial.iter().all(|byte| *byte == 0) {
            return Err(TestFailure::CertificateInvalid);
        }
        Ok(ParsedLeafProfile { serial })
    }

    fn parsed_leaf_serial(leaf_der: &[u8]) -> Result<Vec<u8>, TestFailure> {
        let (remainder, certificate) =
            parse_x509_certificate(leaf_der).map_err(|_| TestFailure::CertificateInvalid)?;
        if !remainder.is_empty() || certificate.raw_serial().is_empty() {
            return Err(TestFailure::CertificateInvalid);
        }
        Ok(certificate.raw_serial().to_vec())
    }

    fn expect_issued(
        result: Result<EnrollmentOutcome, EnrollmentError>,
    ) -> Result<enrollment::IssuedEnrollment, TestFailure> {
        match result.map_err(|_| TestFailure::IntakeFailed)? {
            EnrollmentOutcome::Issued(issued) => Ok(issued),
            EnrollmentOutcome::Pending(_) => Err(TestFailure::UnexpectedOutcome),
        }
    }

    fn expect_pending(
        result: Result<EnrollmentOutcome, EnrollmentError>,
    ) -> Result<Uuid, TestFailure> {
        match result.map_err(|_| TestFailure::IntakeFailed)? {
            EnrollmentOutcome::Pending(pending) => Ok(pending.enrollment_request_id),
            EnrollmentOutcome::Issued(_) => Err(TestFailure::UnexpectedOutcome),
        }
    }

    fn correlation_id() -> CorrelationId {
        CorrelationId::from_uuid(Uuid::now_v7())
    }

    async fn business_counts(database: &Database) -> Result<BusinessCounts, TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                 (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                 (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                 (SELECT COUNT(*) FROM gateway_certificates) AS certificates",
                )
                .get_result(connection)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn complete_counts(database: &Database) -> Result<CompleteCounts, TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                 (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                 (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                 (SELECT COUNT(*) FROM gateway_certificates) AS certificates, \
                 (SELECT COUNT(*) FROM audit_events) AS audits",
                )
                .get_result(connection)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn credential_counts(database: &Database) -> Result<(i64, i64), TestFailure> {
        let counts = business_counts(database).await?;
        Ok((counts.tokens, counts.certificates))
    }

    async fn issuance_evidence(database: &Database) -> Result<IssuanceEvidence, TestFailure> {
        database
        .test_read(|connection| {
            diesel::sql_query(
                "SELECT (SELECT COUNT(*) FROM devices) AS devices, \
                 (SELECT COUNT(*) FROM enrollment_requests) AS requests, \
                 (SELECT COUNT(*) FROM device_tokens) AS tokens, \
                 (SELECT COUNT(*) FROM gateway_certificates) AS certificates, \
                 (SELECT COUNT(*) FROM gateway_certificates WHERE status = 'active') \
                 AS active_certificates, dt.token_hash, gc.spki_sha256 AS certificate_spki, \
                 er.state AS request_state, er.resolution, ae.actor AS audit_actor, \
                 gc.serial AS certificate_serial, ae.action_kind AS audit_action, \
                 ae.reason_code AS audit_reason, \
                 ae.redacted_detail_json AS audit_detail FROM enrollment_requests er \
                 JOIN device_tokens dt ON dt.enrollment_request_id = er.enrollment_request_id \
                 JOIN gateway_certificates gc ON gc.enrollment_request_id = er.enrollment_request_id \
                 JOIN audit_events ae ON ae.audit_event_id = er.issuance_audit_event_id",
            )
            .get_result(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
        })
        .await
        .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn request_state(database: &Database, request_id: Uuid) -> Result<String, TestFailure> {
        database
            .test_read(move |connection| {
                diesel::sql_query(
                "SELECT state AS value FROM enrollment_requests WHERE enrollment_request_id = ?",
            )
            .bind::<Text, _>(request_id.to_string())
            .get_result::<StringRow>(connection)
            .map(|row| row.value)
            .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn device_state(database: &Database, device_id: Uuid) -> Result<String, TestFailure> {
        database
            .test_read(move |connection| {
                diesel::sql_query("SELECT state AS value FROM devices WHERE device_pk = ?")
                    .bind::<Text, _>(device_id.to_string())
                    .get_result::<StringRow>(connection)
                    .map(|row| row.value)
                    .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn assert_reactivation_issuance(
        database: &Database,
        expected_device_id: Uuid,
        issued_device_id: Uuid,
        previous_device_state: &'static str,
    ) -> Result<(), TestFailure> {
        let (actor, reason, encoded_detail) =
            audit_shape(database, "issue_device_credentials").await?;
        let detail: Value =
            serde_json::from_str(&encoded_detail).map_err(|_| TestFailure::EvidenceFailed)?;
        let detail = detail.as_object().ok_or(TestFailure::EvidenceFailed)?;
        if expected_device_id != issued_device_id
            || device_state(database, expected_device_id).await? != "enrolled"
            || actor != "device:enrollment"
            || reason != "credential_replacement"
            || detail.len() != 5
            || detail.get("resolution").and_then(Value::as_str)
                != Some("replace_device_credentials")
            || detail.get("previous_device_state").and_then(Value::as_str)
                != Some(previous_device_state)
            || detail
                .get("evicted_live_connection")
                .and_then(Value::as_bool)
                != Some(false)
            || detail
                .get("certificate_serial")
                .and_then(Value::as_str)
                .is_none_or(|serial| {
                    serial.len() != 40
                        || !serial
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            || detail
                .get("gateway_spki_sha256")
                .and_then(Value::as_str)
                .is_none_or(|digest| digest.len() != 64)
        {
            return Err(TestFailure::LifecycleClaimDidNotReactivate);
        }
        Ok(())
    }

    async fn active_certificate_count_for_device(
        database: &Database,
        device_id: Uuid,
    ) -> Result<i64, TestFailure> {
        database
            .test_read(move |connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM gateway_certificates \
                 WHERE device_pk = ? AND status = 'active'",
                )
                .bind::<Text, _>(device_id.to_string())
                .get_result::<CountRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn seed_live_request_capacity(database: &Database) -> Result<(), TestFailure> {
        database
            .test_write(|connection| {
                diesel::sql_query(
                    "WITH RECURSIVE counter(value) AS ( \
                     SELECT 1 UNION ALL SELECT value + 1 FROM counter WHERE value < ? \
                 ) INSERT INTO enrollment_requests (enrollment_request_id, \
                     machine_hardware_id, hardware_identity_quality, gateway_csr_der, \
                     gateway_spki_sha256, client_version, protocol_version, source_ip, \
                     state, resolution, resolved_device_pk, issuance_audit_event_id, created_at) \
                 SELECT printf('capacity-request-%03d', value), \
                     printf('capacity-hardware-%03d', value), 'strong', x'01', randomblob(32), \
                     'capacity-fixture', 1, '192.0.2.200', 'pending', NULL, NULL, NULL, \
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM counter",
                )
                .bind::<BigInt, _>(MAX_LIVE_ENROLLMENT_REQUESTS)
                .execute(connection)
                .map(|_| ())
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn live_request_count(database: &Database) -> Result<i64, TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM enrollment_requests \
                 WHERE state IN ('pending', 'approved')",
                )
                .get_result::<CountRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn certificate_states(database: &Database) -> Result<(i64, i64), TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT SUM(status = 'active') AS active, SUM(status = 'retired') AS retired \
                 FROM gateway_certificates",
                )
                .get_result::<CertificateStateCounts>(connection)
                .map(|row| (row.active, row.retired))
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn latest_issuance_reason(database: &Database) -> Result<String, TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT reason_code AS value FROM audit_events \
                 WHERE action_kind = 'issue_device_credentials' ORDER BY rowid DESC LIMIT 1",
                )
                .get_result::<StringRow>(connection)
                .map(|row| row.value)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn audit_shape(
        database: &Database,
        action: &'static str,
    ) -> Result<(String, String, String), TestFailure> {
        database
            .test_read(move |connection| {
                diesel::sql_query(
                    "SELECT actor, reason_code AS reason, redacted_detail_json AS detail \
                 FROM audit_events WHERE action_kind = ? ORDER BY rowid DESC LIMIT 1",
                )
                .bind::<Text, _>(action)
                .get_result::<AuditShape>(connection)
                .map(|row| (row.actor, row.reason, row.detail))
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn expiry_audit(database: &Database) -> Result<(i64, String), TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT CAST(json_extract(redacted_detail_json, '$.expired_count') AS INTEGER) \
                 AS expired_count, reason_code AS reason FROM audit_events \
                 WHERE action_kind = 'expire_enrollment_requests' ORDER BY rowid DESC LIMIT 1",
                )
                .get_result::<ExpiryAudit>(connection)
                .map(|row| (row.expired_count, row.reason))
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn reserve_audit_id(database: &Database, id: Uuid) -> Result<(), TestFailure> {
        database
            .test_write(move |connection| {
                diesel::sql_query(
                    "INSERT INTO audit_events (audit_event_id, occurred_at, actor, action_kind, \
                 resource_type, resource_id, result, reason_code, correlation_id, \
                 group_correlation_id, redacted_detail_json) VALUES (?, \
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'system:test', 'reserved_test_audit', \
                 'test', NULL, 'succeeded', NULL, ?, NULL, '{}')",
                )
                .bind::<Text, _>(id.to_string())
                .bind::<Text, _>(Uuid::now_v7().to_string())
                .execute(connection)
                .map(|_| ())
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    async fn install_injected_persistence_failure(
        database: &Database,
        failure: InjectedPersistenceFailure,
    ) -> Result<(), TestFailure> {
        let statement = match failure {
            InjectedPersistenceFailure::Audit => {
                "CREATE TRIGGER injected_enrollment_audit_failure \
             BEFORE INSERT ON audit_events \
             WHEN NEW.action_kind = 'issue_device_credentials' \
             BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;"
            }
            InjectedPersistenceFailure::DeviceToken => {
                "CREATE TRIGGER injected_device_token_failure \
             BEFORE INSERT ON device_tokens \
             BEGIN SELECT RAISE(ABORT, 'injected Device Token failure'); END;"
            }
            InjectedPersistenceFailure::GatewayCertificate => {
                "CREATE TRIGGER injected_gateway_certificate_failure \
             BEFORE INSERT ON gateway_certificates \
             BEGIN SELECT RAISE(ABORT, 'injected Gateway certificate failure'); END;"
            }
        };
        database
            .test_write(move |connection| {
                connection
                    .batch_execute(statement)
                    .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)??;
        Ok(())
    }

    async fn install_claim_cas_ignore(database: &Database) -> Result<(), TestFailure> {
        database
            .test_write(|connection| {
                connection
                    .batch_execute(
                        "CREATE TRIGGER injected_claim_cas_ignore \
                     BEFORE UPDATE OF state ON enrollment_requests \
                     WHEN OLD.state = 'approved' AND NEW.state = 'issued' \
                     BEGIN SELECT RAISE(IGNORE); END;",
                    )
                    .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)??;
        Ok(())
    }

    async fn rollback_snapshot(database: &Database) -> Result<RollbackSnapshot, TestFailure> {
        database
            .test_read(|connection| {
                diesel::sql_query(
                    "SELECT \
                 (SELECT json_group_array(json_object( \
                    'device_pk', device_pk, 'machine_hardware_id', machine_hardware_id, \
                    'hardware_identity_quality', hardware_identity_quality, 'state', state)) \
                  FROM (SELECT * FROM devices ORDER BY device_pk)) AS devices, \
                 (SELECT json_group_array(json_object( \
                    'enrollment_request_id', enrollment_request_id, \
                    'machine_hardware_id', machine_hardware_id, \
                    'hardware_identity_quality', hardware_identity_quality, \
                    'gateway_csr_der', hex(gateway_csr_der), \
                    'gateway_spki_sha256', hex(gateway_spki_sha256), \
                    'client_version', client_version, 'protocol_version', protocol_version, \
                    'source_ip', source_ip, 'state', state, 'resolution', resolution, \
                    'resolved_device_pk', resolved_device_pk, \
                    'issuance_audit_event_id', issuance_audit_event_id, \
                    'created_at', created_at)) \
                  FROM (SELECT * FROM enrollment_requests ORDER BY enrollment_request_id)) \
                    AS requests, \
                 (SELECT json_group_array(json_object( \
                    'device_pk', device_pk, 'enrollment_request_id', enrollment_request_id, \
                    'token_hash', hex(token_hash))) \
                  FROM (SELECT * FROM device_tokens ORDER BY device_pk)) AS tokens, \
                 (SELECT json_group_array(json_object( \
                    'certificate_id', certificate_id, 'device_pk', device_pk, \
                    'enrollment_request_id', enrollment_request_id, 'serial', serial, \
                    'spki_sha256', hex(spki_sha256), 'not_after', not_after, 'status', status)) \
                  FROM (SELECT * FROM gateway_certificates ORDER BY certificate_id)) \
                    AS certificates, \
                 (SELECT json_group_array(json_object( \
                    'audit_event_id', audit_event_id, 'occurred_at', occurred_at, \
                    'actor', actor, 'action_kind', action_kind, \
                    'resource_type', resource_type, 'resource_id', resource_id, \
                    'result', result, 'reason_code', reason_code, \
                    'correlation_id', correlation_id, \
                    'group_correlation_id', group_correlation_id, \
                    'redacted_detail_json', redacted_detail_json)) \
                  FROM (SELECT * FROM audit_events ORDER BY audit_event_id)) AS audits",
                )
                .get_result::<RollbackSnapshot>(connection)
                .map_err(|_| TestFailure::EvidenceFailed)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct BusinessCounts {
        #[diesel(sql_type = BigInt)]
        devices: i64,
        #[diesel(sql_type = BigInt)]
        requests: i64,
        #[diesel(sql_type = BigInt)]
        tokens: i64,
        #[diesel(sql_type = BigInt)]
        certificates: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct CompleteCounts {
        #[diesel(sql_type = BigInt)]
        devices: i64,
        #[diesel(sql_type = BigInt)]
        requests: i64,
        #[diesel(sql_type = BigInt)]
        tokens: i64,
        #[diesel(sql_type = BigInt)]
        certificates: i64,
        #[diesel(sql_type = BigInt)]
        audits: i64,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct RollbackSnapshot {
        #[diesel(sql_type = Text)]
        devices: String,
        #[diesel(sql_type = Text)]
        requests: String,
        #[diesel(sql_type = Text)]
        tokens: String,
        #[diesel(sql_type = Text)]
        certificates: String,
        #[diesel(sql_type = Text)]
        audits: String,
    }

    #[derive(QueryableByName)]
    struct IssuanceEvidence {
        #[diesel(sql_type = BigInt)]
        devices: i64,
        #[diesel(sql_type = BigInt)]
        requests: i64,
        #[diesel(sql_type = BigInt)]
        tokens: i64,
        #[diesel(sql_type = BigInt)]
        certificates: i64,
        #[diesel(sql_type = BigInt)]
        active_certificates: i64,
        #[diesel(sql_type = Binary)]
        token_hash: Vec<u8>,
        #[diesel(sql_type = Binary)]
        certificate_spki: Vec<u8>,
        #[diesel(sql_type = Text)]
        request_state: String,
        #[diesel(sql_type = Text)]
        resolution: String,
        #[diesel(sql_type = Text)]
        audit_actor: String,
        #[diesel(sql_type = Text)]
        certificate_serial: String,
        #[diesel(sql_type = Text)]
        audit_action: String,
        #[diesel(sql_type = Text)]
        audit_reason: String,
        #[diesel(sql_type = Text)]
        audit_detail: String,
    }

    #[derive(QueryableByName)]
    struct StringRow {
        #[diesel(sql_type = Text)]
        value: String,
    }

    #[derive(QueryableByName)]
    struct CertificateStateCounts {
        #[diesel(sql_type = BigInt)]
        active: i64,
        #[diesel(sql_type = BigInt)]
        retired: i64,
    }

    #[derive(QueryableByName)]
    struct AuditShape {
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        reason: String,
        #[diesel(sql_type = Text)]
        detail: String,
    }

    #[derive(QueryableByName)]
    struct ExpiryAudit {
        #[diesel(sql_type = BigInt)]
        expired_count: i64,
        #[diesel(sql_type = Text)]
        reason: String,
    }

    struct DatabaseFixture {
        path: PathBuf,
    }

    impl DatabaseFixture {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir().join(format!(
                    "natsume-enrollment-test-{}.sqlite3",
                    Uuid::now_v7()
                )),
            }
        }

        async fn connect(&self) -> Result<Database, TestFailure> {
            Database::connect_and_migrate(&DatabaseConfig::new(&self.path, true))
                .await
                .map_err(|_| TestFailure::DatabaseFailed)
        }

        fn database_bytes(&self) -> Result<Vec<u8>, TestFailure> {
            let mut bytes = fs::read(&self.path).map_err(|_| TestFailure::EvidenceFailed)?;
            let wal = PathBuf::from(format!("{}-wal", self.path.display()));
            if wal.exists() {
                bytes.extend(fs::read(wal).map_err(|_| TestFailure::EvidenceFailed)?);
            }
            Ok(bytes)
        }
    }

    impl Drop for DatabaseFixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(format!("{}-wal", self.path.display()));
            let _ = fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the test database failed"))]
        DatabaseFailed,
        #[snafu(display("the test issuer failed"))]
        IssuerFailed,
        #[snafu(display("the test request failed"))]
        RequestFailed,
        #[snafu(display("the provisioning window mutation failed"))]
        WindowMutationFailed,
        #[snafu(display("Enrollment intake failed"))]
        IntakeFailed,
        #[snafu(display("the Enrollment outcome was unexpected"))]
        UnexpectedOutcome,
        #[snafu(display("the closed window wrote state"))]
        ClosedWindowWrote,
        #[snafu(display("the issuance evidence changed"))]
        IssuanceEvidenceChanged,
        #[snafu(display("the issued certificate is invalid"))]
        CertificateInvalid,
        #[snafu(display("the issued Gateway certificate profile changed"))]
        GatewayCertificateProfileChanged,
        #[snafu(display("CSR authority escaped into the leaf"))]
        CsrAuthorityEscaped,
        #[snafu(display("issuance plaintext was persisted"))]
        PlaintextPersisted,
        #[snafu(display("the pending replay wrote state"))]
        PendingReplayWrote,
        #[snafu(display("a different-SPKI conflict wrote state"))]
        ConflictWrote,
        #[snafu(display("approval issued credentials"))]
        ApprovalIssued,
        #[snafu(display("window close did not expire the request"))]
        CloseDidNotExpire,
        #[snafu(display("a closed claim issued credentials"))]
        ClosedClaimIssued,
        #[snafu(display("same-SPKI retry semantics changed"))]
        SameSpkiRetryChanged,
        #[snafu(display("rejected polling semantics changed"))]
        RejectedPollChanged,
        #[snafu(display("a rejected hardware identity bypassed rejection by rotating its SPKI"))]
        RejectedKeyRotationWrote,
        #[snafu(display("window close did not clear the rejected hardware block"))]
        RejectedWindowBlockDidNotClear,
        #[snafu(display("the Device lifecycle mutation failed"))]
        LifecycleMutationFailed,
        #[snafu(display("a revoked or disabled Device used same-SPKI auto-approval"))]
        LifecycleReplacementAutoApproved,
        #[snafu(display("the approved lifecycle replacement did not reactivate the Device"))]
        LifecycleClaimDidNotReactivate,
        #[snafu(display("the one-active-certificate invariant changed"))]
        ActiveCertificateInvariantChanged,
        #[snafu(display("the live Enrollment capacity rejection wrote state"))]
        LiveRequestCapacityWrote,
        #[snafu(display("the operator decision failed"))]
        DecisionFailed,
        #[snafu(display("a CSR/SPKI mismatch wrote state"))]
        MismatchWrote,
        #[snafu(display("a duplicate audit did not roll back issuance"))]
        DuplicateAuditDidNotRollBack,
        #[snafu(display("an injected Enrollment persistence failure did not roll back exactly"))]
        InjectedFailureDidNotRollBack,
        #[snafu(display("the approved claim CAS failure did not roll back exactly"))]
        ClaimCasFailureDidNotRollBack,
        #[snafu(display("current credential corruption caused an Enrollment write"))]
        CredentialCorruptionWrote,
        #[snafu(display("database evidence failed"))]
        EvidenceFailed,
    }
}
