use super::{
    CommittedImportFacts, CsvImportErrorCategory, ImportError, PreviewToken, commit_import,
    create_import_candidate, discard_import,
};

#[cfg(test)]
mod candidate_tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use diesel::{
        Connection, QueryableByName, RunQueryDsl,
        connection::SimpleConnection,
        sql_types::{BigInt, Binary, Nullable, Text},
        sqlite::SqliteConnection,
    };
    use sha2::{Digest, Sha256};
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        audit::CorrelationId,
        db::{Database, DatabaseConfig},
        vault::{ensure_master_key, open},
    };

    use super::{
        CsvImportErrorCategory, ImportError, PreviewToken, commit_import, create_import_candidate,
        discard_import,
    };

    const DEVICE_C: &str = "01900000-0000-7000-8000-000000000201";
    const DEVICE_D: &str = "01900000-0000-7000-8000-000000000202";
    const DEVICE_A: &str = "01900000-0000-7000-8000-000000000203";

    #[tokio::test]
    async fn invalid_upload_stops_before_vault_or_database_access() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let mut observer = fixture.observer()?;
        let before = persistence_snapshot(&fixture.database).await?;
        let data_version_before = data_version(&mut observer)?;
        let missing_key_path = fixture.directory_path.join("missing-master.key");

        let Err(error) = create_import_candidate(
            &fixture.database,
            &missing_key_path,
            b"seat,account,password\nA-01,team-a,parse-password-canary,extra",
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::InvalidUploadWasAccepted);
        };
        let ImportError::InvalidCsv(parse_error) = error else {
            return Err(TestFailure::InvalidUploadReachedVault);
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        if parse_error.category() != CsvImportErrorCategory::WrongColumnCount
            || parse_error.line() != 2
            || display.contains("parse-password-canary")
            || debug.contains("parse-password-canary")
        {
            return Err(TestFailure::PendingErrorChanged);
        }

        let after = persistence_snapshot(&fixture.database).await?;
        let data_version_after = data_version(&mut observer)?;
        if before != after || data_version_before != data_version_after || missing_key_path.exists()
        {
            return Err(TestFailure::InvalidUploadWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn candidate_creation_persists_encrypted_payload_and_golden_redacted_diff()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_golden_current_facts(&fixture.database).await?;
        let correlation_id = correlation_id();
        let csv = b"seat,account,password\n\
                    F-06,new-f,password-f\n\
                    E-05,new-e,password-e\n\
                    A-01,same-a,password-a\n\
                    B-02,new-b,password-b\n\
                    G-07,new-g,password-g";

        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            csv,
            correlation_id,
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let evidence = candidate_evidence(&fixture.database).await?;
        let expected_preview = r#"{"seats_added":["F-06","G-07"],"seats_removed":["C-03","D-04"],"mappings_changed":[{"seat_code":"B-02","current_domjudge_username":"old-b","candidate_domjudge_username":"new-b"},{"seat_code":"E-05","current_domjudge_username":null,"candidate_domjudge_username":"new-e"}],"unchanged_count":1,"affected_account_count":5,"binding_impacts":[{"seat_code":"C-03","device_id":"01900000-0000-7000-8000-000000000201"},{"seat_code":"D-04","device_id":"01900000-0000-7000-8000-000000000202"}]}"#;
        let expected_staging = br#"[{"seat_code":"F-06","domjudge_username":"new-f","password":"password-f"},{"seat_code":"E-05","domjudge_username":"new-e","password":"password-e"},{"seat_code":"A-01","domjudge_username":"same-a","password":"password-a"},{"seat_code":"B-02","domjudge_username":"new-b","password":"password-b"},{"seat_code":"G-07","domjudge_username":"new-g","password":"password-g"}]"#;

        if created.candidate_id().to_string() != evidence.candidate_id
            || created.expires_at() != evidence.expires_at
            || created.baseline_configuration_revision() != 23
            || created.baseline_binding_revision() != 31
            || evidence.baseline_configuration_revision != 23
            || evidence.baseline_binding_revision != 31
            || evidence.ttl_valid != 1
            || evidence.record_type != "import_payload"
            || evidence.subject_id != evidence.candidate_id
            || evidence.nonce.len() != 24
            || evidence.redacted_preview_json != expected_preview
            || serde_json::to_string(created.diff()).map_err(|_| TestFailure::EvidenceFailed)?
                != expected_preview
        {
            return Err(TestFailure::CandidateEvidenceChanged);
        }
        assert_canonical_uuid_v7(&evidence.candidate_id)?;
        assert_canonical_uuid_v7(&evidence.payload_vault_record_id)?;

        let expected_hash = Sha256::digest(created.preview_token().as_bytes());
        if evidence.preview_token_hash.as_slice() != expected_hash.as_slice() {
            return Err(TestFailure::PreviewTokenHashChanged);
        }
        assert_database_files_exclude(&fixture.database_path, created.preview_token().as_bytes())?;

        let opened = open(
            &fixture.master_key_path,
            &evidence.nonce,
            &evidence.ciphertext,
        )
        .map_err(|_| TestFailure::PayloadOpenFailed)?;
        if opened.as_slice() != expected_staging
            || evidence.ciphertext.as_slice() == expected_staging
        {
            return Err(TestFailure::PayloadEvidenceChanged);
        }
        assert_database_files_exclude(&fixture.database_path, expected_staging)?;

        let audit = audit_for_resource(&fixture.database, &evidence.candidate_id).await?;
        if audit.actor != "operator:self"
            || audit.action_kind != "create_import_candidate"
            || audit.resource_type != "import_candidate"
            || audit.resource_id.as_deref() != Some(evidence.candidate_id.as_str())
            || audit.result != "succeeded"
            || audit.reason_code.as_deref() != Some("operator_requested")
            || audit.correlation_id != correlation_id.as_text()
            || audit.redacted_detail_json
                != r#"{"seats_added_count":2,"seats_removed_count":2,"mappings_changed_count":2,"binding_impact_count":2}"#
        {
            return Err(TestFailure::AuditEvidenceChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn second_upload_while_pending_commits_zero_writes() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let first = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,first-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let before = persistence_snapshot(&fixture.database).await?;
        let mut observer = fixture.observer()?;
        let data_version_before = data_version(&mut observer)?;

        let Err(error) = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,pending-password-canary",
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::PendingCandidateWasReplaced);
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        if error != ImportError::CandidatePending
            || display.contains("pending-password-canary")
            || debug.contains("pending-password-canary")
        {
            return Err(TestFailure::PendingErrorChanged);
        }

        let after = persistence_snapshot(&fixture.database).await?;
        let data_version_after = data_version(&mut observer)?;
        if before != after
            || data_version_before != data_version_after
            || after.candidate_id.as_deref() != Some(first.candidate_id().to_string().as_str())
        {
            return Err(TestFailure::PendingUploadWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn expired_pending_candidate_is_replaced_atomically_in_one_upload()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let first = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,old-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let old = candidate_evidence(&fixture.database).await?;
        expire_current_candidate(&fixture.database).await?;

        let expiry_correlation = correlation_id();
        let second = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,new-password",
            expiry_correlation,
        )
        .await
        .map_err(|_| TestFailure::ExpiredReplacementFailed)?;
        let current = candidate_evidence(&fixture.database).await?;
        if first.candidate_id() == second.candidate_id()
            || old.candidate_id == current.candidate_id
            || current.candidate_id != second.candidate_id().to_string()
            || vault_record_count(&fixture.database, &old.payload_vault_record_id).await? != 0
            || vault_record_count(&fixture.database, &current.payload_vault_record_id).await? != 1
        {
            return Err(TestFailure::ExpiredCandidateWasNotReplaced);
        }

        let expiry_audit = expiry_audit(&fixture.database, &old.candidate_id).await?;
        if expiry_audit.count != 1
            || expiry_audit.actor != "system:expiry"
            || expiry_audit.action_kind != "expire_import_candidate"
            || expiry_audit.resource_type != "import_candidate"
            || expiry_audit.resource_id.as_deref() != Some(old.candidate_id.as_str())
            || expiry_audit.result != "succeeded"
            || expiry_audit.reason_code.as_deref() != Some("absolute_expiry_observed")
            || expiry_audit.correlation_id != expiry_correlation.as_text()
            || expiry_audit.redacted_detail_json != "{}"
            || import_audit_count(&fixture.database).await? != 3
        {
            return Err(TestFailure::ExpiryAuditChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn upload_recovers_expired_candidate_with_missing_payload() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,missing-expired-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let old = candidate_evidence(&fixture.database).await?;
        expire_current_candidate(&fixture.database).await?;
        delete_payload_out_of_band(&fixture, &old.payload_vault_record_id)?;

        let replacement = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,replacement-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::ExpiredReplacementFailed)?;
        let current = candidate_evidence(&fixture.database).await?;
        if current.candidate_id != replacement.candidate_id().to_string()
            || current.candidate_id == old.candidate_id
            || expiry_audit(&fixture.database, &old.candidate_id)
                .await?
                .count
                != 1
        {
            return Err(TestFailure::ExpiredCandidateWasNotReplaced);
        }
        Ok(())
    }

    #[tokio::test]
    async fn identical_candidate_has_an_empty_non_secret_diff() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_identical_current_facts(&fixture.database).await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,new-b-password\nA-01,team-a,new-a-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let expected = r#"{"seats_added":[],"seats_removed":[],"mappings_changed":[],"unchanged_count":2,"affected_account_count":2,"binding_impacts":[]}"#;
        let serialized =
            serde_json::to_string(created.diff()).map_err(|_| TestFailure::EvidenceFailed)?;
        let persisted = candidate_evidence(&fixture.database).await?;
        if serialized != expected
            || persisted.redacted_preview_json != expected
            || !created.diff().seats_added().is_empty()
            || !created.diff().seats_removed().is_empty()
            || !created.diff().mappings_changed().is_empty()
            || created.diff().unchanged_count() != 2
            || created.diff().affected_account_count() != 2
            || !created.diff().binding_impacts().is_empty()
        {
            return Err(TestFailure::EmptyDiffChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn material_commit_atomically_replaces_configuration_and_credentials()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_golden_current_facts(&fixture.database).await?;
        seed_surviving_binding(&fixture.database).await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\n\
              A-01,old-b,material-password-old-b\n\
              B-02,same-a,material-password-same-a\n\
              F-06,new-f,material-password-new-f",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let candidate = candidate_evidence(&fixture.database).await?;
        let token_bytes = *created.preview_token().as_bytes();

        let committed = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            created.candidate_id(),
            created.preview_token(),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CommitFailed)?;
        let state = committed_state(&fixture.database).await?;
        let old_b = account_for_username(&state.accounts, "old-b")?;
        let same_a = account_for_username(&state.accounts, "same-a")?;
        let new_f = account_for_username(&state.accounts, "new-f")?;

        if committed.configuration_revision() != 24
            || committed.binding_revision() != 32
            || state.revisions != RevisionEvidence::new(24, 32)
            || state.seats
                != vec![
                    SeatEvidence::new("A-01", "seat-a", Some("account-b"), Some("old-b")),
                    SeatEvidence::new("B-02", "seat-b", Some("account-a"), Some("same-a")),
                    SeatEvidence::new(
                        "F-06",
                        "F-06",
                        Some(new_f.account_id.as_str()),
                        Some("new-f"),
                    ),
                ]
            || state.bindings != vec![BindingEvidence::new("A-01", DEVICE_A, 19)]
            || state.accounts.len() != 3
            || !account_nonces_are_pairwise_distinct(&state.accounts)
            || state.candidate_count != 0
            || vault_record_count(&fixture.database, &candidate.payload_vault_record_id).await? != 0
        {
            return Err(TestFailure::MaterialCommitChanged);
        }

        if old_b.account_id != "account-b"
            || old_b.credential_vault_record_id != "vault-b"
            || old_b.credential_revision != 4
            || old_b.nonce == vec![0x02]
            || old_b.ciphertext == vec![0x12]
            || same_a.account_id != "account-a"
            || same_a.credential_vault_record_id != "vault-a"
            || same_a.credential_revision != 3
            || same_a.nonce == vec![0x01]
            || same_a.ciphertext == vec![0x11]
            || new_f.credential_revision != 1
            || state.accounts.iter().any(|account| {
                matches!(
                    account.domjudge_username.as_str(),
                    "old-c" | "old-d" | "old-e"
                )
            })
            || vault_record_count(&fixture.database, "vault-c").await? != 0
            || vault_record_count(&fixture.database, "vault-d").await? != 0
            || vault_record_count(&fixture.database, "vault-e").await? != 0
        {
            return Err(TestFailure::CredentialReplacementChanged);
        }
        assert_canonical_uuid_v7(&new_f.account_id)?;
        assert_canonical_uuid_v7(&new_f.credential_vault_record_id)?;
        assert_opened_credential(&fixture.master_key_path, old_b, b"material-password-old-b")?;
        assert_opened_credential(
            &fixture.master_key_path,
            same_a,
            b"material-password-same-a",
        )?;
        assert_opened_credential(&fixture.master_key_path, new_f, b"material-password-new-f")?;

        assert_material_commit_audit(&fixture.database, &candidate.candidate_id).await?;
        assert_database_files_exclude_all(
            &fixture.database_path,
            &[
                b"material-password-old-b",
                b"material-password-same-a",
                b"material-password-new-f",
                token_bytes.as_slice(),
            ],
        )?;
        Ok(())
    }

    #[tokio::test]
    async fn no_op_commit_only_rotates_credentials() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_identical_current_facts(&fixture.database).await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,no-op-password-b\nA-01,team-a,no-op-password-a",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let candidate_id = created.candidate_id().to_string();
        let commit_correlation = correlation_id();
        let committed = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            created.candidate_id(),
            created.preview_token(),
            commit_correlation,
        )
        .await
        .map_err(|_| TestFailure::CommitFailed)?;
        let state = committed_state(&fixture.database).await?;
        if committed != super::CommittedImportFacts::new(7, 9)
            || state.revisions != RevisionEvidence::new(7, 9)
            || account_for_username(&state.accounts, "team-a")?.credential_revision != 4
            || account_for_username(&state.accounts, "team-b")?.credential_revision != 5
            || state.bindings != vec![BindingEvidence::new("A-01", DEVICE_A, 9)]
            || state.candidate_count != 0
        {
            return Err(TestFailure::NoOpCommitChanged);
        }
        let audit = audit_for_action(
            &fixture.database,
            &candidate_id,
            "commit_import",
            "succeeded",
        )
        .await?;
        if audit.correlation_id != commit_correlation.as_text()
            || audit.redacted_detail_json
                != r#"{"seats_added_count":0,"seats_removed_count":0,"mappings_changed_count":0,"binding_impact_count":0,"credential_revision_advanced_count":2,"configuration_revision_advanced":false,"binding_revision_advanced":false}"#
        {
            return Err(TestFailure::CommitAuditChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn mapping_only_commit_advances_configuration_without_binding_revision()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_identical_current_facts(&fixture.database).await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-b,mapping-password-b\nB-02,team-a,mapping-password-a",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let candidate_id = created.candidate_id().to_string();
        let committed = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            created.candidate_id(),
            created.preview_token(),
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CommitFailed)?;
        let state = committed_state(&fixture.database).await?;
        if committed != super::CommittedImportFacts::new(8, 9)
            || state.revisions != RevisionEvidence::new(8, 9)
            || state.seats
                != vec![
                    SeatEvidence::new(
                        "A-01",
                        "same-seat-a",
                        Some("same-account-b"),
                        Some("team-b"),
                    ),
                    SeatEvidence::new(
                        "B-02",
                        "same-seat-b",
                        Some("same-account-a"),
                        Some("team-a"),
                    ),
                ]
            || state.bindings != vec![BindingEvidence::new("A-01", DEVICE_A, 9)]
            || state.candidate_count != 0
        {
            return Err(TestFailure::MappingOnlyCommitChanged);
        }
        let audit = audit_for_action(
            &fixture.database,
            &candidate_id,
            "commit_import",
            "succeeded",
        )
        .await?;
        if audit.redacted_detail_json
            != r#"{"seats_added_count":0,"seats_removed_count":0,"mappings_changed_count":2,"binding_impact_count":0,"credential_revision_advanced_count":2,"configuration_revision_advanced":true,"binding_revision_advanced":false}"#
        {
            return Err(TestFailure::CommitAuditChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn stale_commit_writes_only_a_rejected_audit_and_keeps_candidate()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_identical_current_facts(&fixture.database).await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,stale-password-a\nB-02,team-b,stale-password-b",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        bump_configuration_revision(&fixture.database).await?;
        let before = committed_state(&fixture.database).await?;
        let before_audits = audit_count(&fixture.database).await?;
        let mut observer = fixture.observer()?;
        let data_version_before = data_version(&mut observer)?;

        let Err(error) = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            created.candidate_id(),
            created.preview_token(),
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::StaleCommitSucceeded);
        };
        let after = committed_state(&fixture.database).await?;
        let data_version_after = data_version(&mut observer)?;
        if error != ImportError::PreviewStale
            || before != after
            || audit_count(&fixture.database).await? != before_audits + 1
            || data_version_after == data_version_before
        {
            return Err(TestFailure::StaleCommitChangedBusinessFacts);
        }
        assert_rejected_commit_audit(
            &fixture.database,
            &created.candidate_id().to_string(),
            "baseline_stale",
        )
        .await
    }

    #[tokio::test]
    async fn token_mismatch_is_unavailable_and_keeps_candidate() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,mismatch-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let before = persistence_snapshot(&fixture.database).await?;
        let wrong_token = PreviewToken::from_bytes([0x5a; 32]);

        let Err(error) = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            created.candidate_id(),
            &wrong_token,
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::MismatchedTokenSucceeded);
        };
        let after = persistence_snapshot(&fixture.database).await?;
        if error != ImportError::CandidateUnavailable
            || before.candidate_count != after.candidate_count
            || before.vault_count != after.vault_count
            || before.candidate_id != after.candidate_id
            || after.audit_count != before.audit_count + 1
        {
            return Err(TestFailure::MismatchedTokenChangedCandidate);
        }
        assert_rejected_commit_audit(
            &fixture.database,
            &created.candidate_id().to_string(),
            "preview_token_mismatch",
        )
        .await
    }

    #[tokio::test]
    async fn commit_observation_lazily_expires_candidate() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,expired-commit-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let candidate = candidate_evidence(&fixture.database).await?;
        expire_current_candidate(&fixture.database).await?;
        let Err(error) = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            created.candidate_id(),
            created.preview_token(),
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::ExpiredOperationSucceeded);
        };
        if error != ImportError::CandidateUnavailable
            || persistence_snapshot(&fixture.database)
                .await?
                .candidate_count
                != 0
            || vault_record_count(&fixture.database, &candidate.payload_vault_record_id).await? != 0
            || expiry_audit(&fixture.database, &candidate.candidate_id)
                .await?
                .count
                != 1
        {
            return Err(TestFailure::ExpiryOperationChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn discard_observation_lazily_expires_candidate() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,expired-discard-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let candidate = candidate_evidence(&fixture.database).await?;
        expire_current_candidate(&fixture.database).await?;
        let Err(error) =
            discard_import(&fixture.database, created.candidate_id(), correlation_id()).await
        else {
            return Err(TestFailure::ExpiredOperationSucceeded);
        };
        if error != ImportError::CandidateUnavailable
            || persistence_snapshot(&fixture.database)
                .await?
                .candidate_count
                != 0
            || vault_record_count(&fixture.database, &candidate.payload_vault_record_id).await? != 0
            || expiry_audit(&fixture.database, &candidate.candidate_id)
                .await?
                .count
                != 1
        {
            return Err(TestFailure::ExpiryOperationChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn unknown_commit_and_repeat_discard_are_zero_write() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,discard-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let before_unknown = persistence_snapshot(&fixture.database).await?;
        let mut observer = fixture.observer()?;
        let version_before_unknown = data_version(&mut observer)?;
        let unknown_id = Uuid::now_v7();
        let unknown_token = PreviewToken::from_bytes([0x6b; 32]);
        let Err(error) = commit_import(
            &fixture.database,
            &fixture.master_key_path,
            unknown_id,
            &unknown_token,
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::UnknownOperationSucceeded);
        };
        if error != ImportError::CandidateUnavailable
            || persistence_snapshot(&fixture.database).await? != before_unknown
            || data_version(&mut observer)? != version_before_unknown
        {
            return Err(TestFailure::UnknownOperationWroteData);
        }

        let discard_correlation = correlation_id();
        discard_import(
            &fixture.database,
            created.candidate_id(),
            discard_correlation,
        )
        .await
        .map_err(|_| TestFailure::DiscardFailed)?;
        let discard_audit = audit_for_action(
            &fixture.database,
            &created.candidate_id().to_string(),
            "discard_import_candidate",
            "succeeded",
        )
        .await?;
        if discard_audit.actor != "operator:self"
            || discard_audit.reason_code.as_deref() != Some("operator_requested")
            || discard_audit.correlation_id != discard_correlation.as_text()
            || discard_audit.redacted_detail_json != "{}"
        {
            return Err(TestFailure::DiscardAuditChanged);
        }
        let before_repeat = persistence_snapshot(&fixture.database).await?;
        let version_before_repeat = data_version(&mut observer)?;
        let Err(error) =
            discard_import(&fixture.database, created.candidate_id(), correlation_id()).await
        else {
            return Err(TestFailure::UnknownOperationSucceeded);
        };
        if error != ImportError::CandidateUnavailable
            || persistence_snapshot(&fixture.database).await? != before_repeat
            || data_version(&mut observer)? != version_before_repeat
        {
            return Err(TestFailure::UnknownOperationWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn discard_recovers_candidate_with_missing_payload() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,wedged-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let candidate = candidate_evidence(&fixture.database).await?;
        delete_payload_out_of_band(&fixture, &candidate.payload_vault_record_id)?;
        discard_import(&fixture.database, created.candidate_id(), correlation_id())
            .await
            .map_err(|_| TestFailure::DiscardFailed)?;
        let snapshot = persistence_snapshot(&fixture.database).await?;
        let audit = audit_for_action(
            &fixture.database,
            &candidate.candidate_id,
            "discard_import_candidate",
            "succeeded",
        )
        .await?;
        if snapshot.candidate_count != 0
            || vault_record_count(&fixture.database, &candidate.payload_vault_record_id).await? != 0
            || audit.redacted_detail_json != "{}"
        {
            return Err(TestFailure::WedgedDiscardFailed);
        }
        Ok(())
    }

    async fn seed_golden_current_facts(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                connection.batch_execute(&format!(
                    "UPDATE revision_counters \
                     SET configuration_revision = 23, binding_revision = 31 WHERE singleton = 1; \
                     INSERT INTO server_vault_records \
                     (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                     ('vault-a', 'account_credential', 'account-a', x'01', x'11'), \
                     ('vault-b', 'account_credential', 'account-b', x'02', x'12'), \
                     ('vault-c', 'account_credential', 'account-c', x'03', x'13'), \
                     ('vault-d', 'account_credential', 'account-d', x'04', x'14'), \
                     ('vault-e', 'account_credential', 'account-e', x'05', x'15'); \
                     INSERT INTO seats (seat_id, seat_code) VALUES \
                     ('seat-d', 'D-04'), ('seat-b', 'B-02'), ('seat-e', 'E-05'), \
                     ('seat-a', 'A-01'), ('seat-c', 'C-03'); \
                     INSERT INTO accounts \
                     (account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES \
                     ('account-a', 'same-a', 'vault-a', 2), \
                     ('account-b', 'old-b', 'vault-b', 3), \
                     ('account-c', 'old-c', 'vault-c', 4), \
                     ('account-d', 'old-d', 'vault-d', 5), \
                     ('account-e', 'old-e', 'vault-e', 6); \
                     INSERT INTO account_mappings (seat_id, account_id) VALUES \
                     ('seat-d', 'account-d'), ('seat-b', 'account-b'), \
                     ('seat-a', 'account-a'), ('seat-c', 'account-c'); \
                     INSERT INTO devices \
                     (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                     ('{DEVICE_D}', 'machine-d', 'strong', 'enrolled'), \
                     ('{DEVICE_C}', 'machine-c', 'medium', 'enrolled'); \
                     INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                     ('seat-d', '{DEVICE_D}', 29), ('seat-c', '{DEVICE_C}', 17);"
                ))
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn seed_identical_current_facts(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                connection.batch_execute(&format!(
                    "UPDATE revision_counters \
                     SET configuration_revision = 7, binding_revision = 9 WHERE singleton = 1; \
                     INSERT INTO server_vault_records \
                     (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                     ('same-vault-a', 'account_credential', 'same-account-a', x'01', x'11'), \
                     ('same-vault-b', 'account_credential', 'same-account-b', x'02', x'12'); \
                     INSERT INTO seats (seat_id, seat_code) VALUES \
                     ('same-seat-b', 'B-02'), ('same-seat-a', 'A-01'); \
                     INSERT INTO accounts \
                     (account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES \
                     ('same-account-b', 'team-b', 'same-vault-b', 4), \
                     ('same-account-a', 'team-a', 'same-vault-a', 3); \
                     INSERT INTO account_mappings (seat_id, account_id) VALUES \
                     ('same-seat-b', 'same-account-b'), ('same-seat-a', 'same-account-a'); \
                     INSERT INTO devices \
                     (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                     ('{DEVICE_A}', 'machine-a', 'strong', 'enrolled'); \
                     INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                     ('same-seat-a', '{DEVICE_A}', 9);"
                ))
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn seed_surviving_binding(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                connection.batch_execute(&format!(
                    "INSERT INTO devices \
                     (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                     ('{DEVICE_A}', 'machine-a-surviving', 'strong', 'enrolled'); \
                     INSERT INTO device_bindings (seat_id, device_pk, binding_revision) \
                     VALUES ('seat-a', '{DEVICE_A}', 19);"
                ))
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn bump_configuration_revision(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "UPDATE revision_counters SET configuration_revision = \
                     configuration_revision + 1 WHERE singleton = 1",
                )
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    fn delete_payload_out_of_band(
        fixture: &ImportFixture,
        payload_vault_record_id: &str,
    ) -> Result<(), TestFailure> {
        let mut connection = fixture.observer()?;
        connection
            .batch_execute("PRAGMA foreign_keys = OFF")
            .map_err(|_| TestFailure::FixtureFailed)?;
        diesel::sql_query("DELETE FROM server_vault_records WHERE vault_record_id = ?")
            .bind::<Text, _>(payload_vault_record_id)
            .execute(&mut connection)
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn committed_state(database: &Database) -> Result<CommittedState, TestFailure> {
        database
            .interact(|connection| {
                let revisions = diesel::sql_query(
                    "SELECT configuration_revision, binding_revision \
                     FROM revision_counters WHERE singleton = 1",
                )
                .get_result::<RevisionEvidence>(connection)?;
                let seats = diesel::sql_query(
                    "SELECT s.seat_code, s.seat_id, m.account_id, a.domjudge_username \
                     FROM seats s \
                     LEFT JOIN account_mappings m ON m.seat_id = s.seat_id \
                     LEFT JOIN accounts a ON a.account_id = m.account_id \
                     ORDER BY s.seat_code",
                )
                .load::<SeatEvidence>(connection)?;
                let accounts = diesel::sql_query(
                    "SELECT a.domjudge_username, a.account_id, a.credential_vault_record_id, \
                     a.credential_revision, v.record_type, v.subject_id, v.nonce, v.ciphertext \
                     FROM accounts a \
                     JOIN server_vault_records v \
                       ON v.vault_record_id = a.credential_vault_record_id \
                     ORDER BY a.domjudge_username",
                )
                .load::<AccountEvidence>(connection)?;
                let bindings = diesel::sql_query(
                    "SELECT s.seat_code, b.device_pk, b.binding_revision \
                     FROM device_bindings b JOIN seats s ON s.seat_id = b.seat_id \
                     ORDER BY s.seat_code",
                )
                .load::<BindingEvidence>(connection)?;
                let candidate_count =
                    diesel::sql_query("SELECT COUNT(*) AS value FROM pending_import_candidate")
                        .get_result::<CountRow>(connection)?
                        .value;
                Ok::<CommittedState, diesel::result::Error>(CommittedState {
                    revisions,
                    seats,
                    accounts,
                    bindings,
                    candidate_count,
                })
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    fn account_for_username<'a>(
        accounts: &'a [AccountEvidence],
        username: &str,
    ) -> Result<&'a AccountEvidence, TestFailure> {
        accounts
            .iter()
            .find(|account| account.domjudge_username == username)
            .ok_or(TestFailure::EvidenceFailed)
    }

    fn account_nonces_are_pairwise_distinct(accounts: &[AccountEvidence]) -> bool {
        accounts.iter().enumerate().all(|(index, account)| {
            accounts[index + 1..]
                .iter()
                .all(|other| account.nonce != other.nonce)
        })
    }

    fn assert_opened_credential(
        master_key_path: &Path,
        account: &AccountEvidence,
        expected: &[u8],
    ) -> Result<(), TestFailure> {
        if account.record_type != "account_credential"
            || account.subject_id != account.account_id
            || account.nonce.len() != 24
        {
            return Err(TestFailure::CredentialReplacementChanged);
        }
        let opened = open(master_key_path, &account.nonce, &account.ciphertext)
            .map_err(|_| TestFailure::PayloadOpenFailed)?;
        if opened.as_slice() != expected {
            return Err(TestFailure::CredentialReplacementChanged);
        }
        Ok(())
    }

    async fn audit_for_action(
        database: &Database,
        resource_id: &str,
        action_kind: &str,
        result: &str,
    ) -> Result<AuditEvidence, TestFailure> {
        let resource_id = resource_id.to_owned();
        let action_kind = action_kind.to_owned();
        let result = result.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                     correlation_id, redacted_detail_json FROM audit_events \
                     WHERE resource_id = ? AND action_kind = ? AND result = ?",
                )
                .bind::<Text, _>(resource_id)
                .bind::<Text, _>(action_kind)
                .bind::<Text, _>(result)
                .get_result::<AuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn assert_rejected_commit_audit(
        database: &Database,
        candidate_id: &str,
        reason: &str,
    ) -> Result<(), TestFailure> {
        let audit = audit_for_action(database, candidate_id, "commit_import", "rejected").await?;
        if audit.actor != "operator:self"
            || audit.action_kind != "commit_import"
            || audit.resource_type != "import_candidate"
            || audit.resource_id.as_deref() != Some(candidate_id)
            || audit.result != "rejected"
            || audit.reason_code.as_deref() != Some(reason)
            || audit.redacted_detail_json != "{}"
        {
            return Err(TestFailure::CommitAuditChanged);
        }
        Ok(())
    }

    async fn assert_material_commit_audit(
        database: &Database,
        candidate_id: &str,
    ) -> Result<(), TestFailure> {
        let audit = audit_for_action(database, candidate_id, "commit_import", "succeeded").await?;
        if audit.actor != "operator:self"
            || audit.resource_type != "import_candidate"
            || audit.resource_id.as_deref() != Some(candidate_id)
            || audit.reason_code.as_deref() != Some("operator_requested")
            || audit.redacted_detail_json
                != r#"{"seats_added_count":1,"seats_removed_count":3,"mappings_changed_count":2,"binding_impact_count":2,"credential_revision_advanced_count":3,"configuration_revision_advanced":true,"binding_revision_advanced":true}"#
        {
            return Err(TestFailure::CommitAuditChanged);
        }
        Ok(())
    }

    async fn audit_count(database: &Database) -> Result<i64, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query("SELECT COUNT(*) AS value FROM audit_events")
                    .get_result::<CountRow>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map(|row| row.value)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn candidate_evidence(database: &Database) -> Result<CandidateEvidence, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT p.candidate_id, p.expires_at, \
                     p.baseline_configuration_revision, p.baseline_binding_revision, \
                     p.preview_token_hash, p.payload_vault_record_id, \
                     p.redacted_preview_json, v.record_type, v.subject_id, v.nonce, v.ciphertext, \
                     CAST(p.expires_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+1799 seconds') \
                       AND p.expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+1800 seconds') \
                       AS INTEGER) AS ttl_valid \
                     FROM pending_import_candidate p \
                     JOIN server_vault_records v \
                       ON v.vault_record_id = p.payload_vault_record_id \
                     WHERE p.singleton = 1",
                )
                .get_result::<CandidateEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn audit_for_resource(
        database: &Database,
        resource_id: &str,
    ) -> Result<AuditEvidence, TestFailure> {
        let resource_id = resource_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                     correlation_id, redacted_detail_json \
                     FROM audit_events WHERE resource_id = ? \
                     AND action_kind = 'create_import_candidate'",
                )
                .bind::<Text, _>(resource_id)
                .get_result::<AuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn expiry_audit(
        database: &Database,
        resource_id: &str,
    ) -> Result<ExpiryAuditEvidence, TestFailure> {
        let resource_id = resource_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS count, actor, action_kind, resource_type, resource_id, \
                     result, reason_code, correlation_id, redacted_detail_json \
                     FROM audit_events WHERE resource_id = ? \
                     AND action_kind = 'expire_import_candidate'",
                )
                .bind::<Text, _>(resource_id)
                .get_result::<ExpiryAuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn expire_current_candidate(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "UPDATE pending_import_candidate \
                     SET expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second') \
                     WHERE singleton = 1",
                )
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn vault_record_count(
        database: &Database,
        vault_record_id: &str,
    ) -> Result<i64, TestFailure> {
        let vault_record_id = vault_record_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM server_vault_records \
                     WHERE vault_record_id = ?",
                )
                .bind::<Text, _>(vault_record_id)
                .get_result::<CountRow>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map(|row| row.value)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn import_audit_count(database: &Database) -> Result<i64, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM audit_events \
                     WHERE action_kind IN \
                     ('create_import_candidate', 'expire_import_candidate')",
                )
                .get_result::<CountRow>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map(|row| row.value)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn persistence_snapshot(database: &Database) -> Result<PersistenceSnapshot, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT \
                     (SELECT COUNT(*) FROM pending_import_candidate) AS candidate_count, \
                     (SELECT COUNT(*) FROM server_vault_records) AS vault_count, \
                     (SELECT COUNT(*) FROM audit_events) AS audit_count, \
                     (SELECT candidate_id FROM pending_import_candidate WHERE singleton = 1) \
                       AS candidate_id",
                )
                .get_result::<PersistenceSnapshot>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    fn data_version(connection: &mut SqliteConnection) -> Result<i64, TestFailure> {
        diesel::dsl::sql::<BigInt>("PRAGMA data_version")
            .get_result(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    fn assert_database_files_exclude(path: &Path, canary: &[u8]) -> Result<(), TestFailure> {
        let database_bytes = fs::read(path).map_err(|_| TestFailure::EvidenceFailed)?;
        if contains_bytes(&database_bytes, canary) {
            return Err(TestFailure::DatabaseLeakedSecret);
        }
        let wal_path = PathBuf::from(format!("{}-wal", path.display()));
        if wal_path.exists() {
            let wal_bytes = fs::read(wal_path).map_err(|_| TestFailure::EvidenceFailed)?;
            if contains_bytes(&wal_bytes, canary) {
                return Err(TestFailure::DatabaseLeakedSecret);
            }
        }
        Ok(())
    }

    fn assert_database_files_exclude_all(
        path: &Path,
        canaries: &[&[u8]],
    ) -> Result<(), TestFailure> {
        for canary in canaries {
            assert_database_files_exclude(path, canary)?;
        }
        Ok(())
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    fn assert_canonical_uuid_v7(value: &str) -> Result<(), TestFailure> {
        let parsed = Uuid::parse_str(value).map_err(|_| TestFailure::CandidateEvidenceChanged)?;
        if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
            return Err(TestFailure::CandidateEvidenceChanged);
        }
        Ok(())
    }

    fn correlation_id() -> CorrelationId {
        CorrelationId::from_uuid(Uuid::now_v7())
    }

    struct ImportFixture {
        database: Database,
        database_path: PathBuf,
        master_key_path: PathBuf,
        directory_path: PathBuf,
    }

    impl ImportFixture {
        async fn new() -> Result<Self, TestFailure> {
            let directory_path =
                std::env::temp_dir().join(format!("natsume-server-import-test-{}", Uuid::now_v7()));
            fs::create_dir(&directory_path).map_err(|_| TestFailure::FixtureFailed)?;
            fs::set_permissions(&directory_path, fs::Permissions::from_mode(0o700))
                .map_err(|_| TestFailure::FixtureFailed)?;
            let database_path = directory_path.join("server.sqlite3");
            let master_key_path = directory_path.join("master.key");
            ensure_master_key(&master_key_path).map_err(|_| TestFailure::FixtureFailed)?;
            let database =
                Database::connect_and_migrate(&DatabaseConfig::new(&database_path, true))
                    .await
                    .map_err(|_| TestFailure::FixtureFailed)?;
            Ok(Self {
                database,
                database_path,
                master_key_path,
                directory_path,
            })
        }

        fn observer(&self) -> Result<SqliteConnection, TestFailure> {
            let path = self
                .database_path
                .to_str()
                .ok_or(TestFailure::FixtureFailed)?;
            SqliteConnection::establish(path).map_err(|_| TestFailure::FixtureFailed)
        }
    }

    impl Drop for ImportFixture {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.directory_path);
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CommittedState {
        revisions: RevisionEvidence,
        seats: Vec<SeatEvidence>,
        accounts: Vec<AccountEvidence>,
        bindings: Vec<BindingEvidence>,
        candidate_count: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct RevisionEvidence {
        #[diesel(sql_type = BigInt)]
        configuration_revision: i64,
        #[diesel(sql_type = BigInt)]
        binding_revision: i64,
    }

    impl RevisionEvidence {
        const fn new(configuration_revision: i64, binding_revision: i64) -> Self {
            Self {
                configuration_revision,
                binding_revision,
            }
        }
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct SeatEvidence {
        #[diesel(sql_type = Text)]
        seat_code: String,
        #[diesel(sql_type = Text)]
        seat_id: String,
        #[diesel(sql_type = Nullable<Text>)]
        account_id: Option<String>,
        #[diesel(sql_type = Nullable<Text>)]
        domjudge_username: Option<String>,
    }

    impl SeatEvidence {
        fn new(
            seat_code: &str,
            seat_id: &str,
            account_id: Option<&str>,
            domjudge_username: Option<&str>,
        ) -> Self {
            Self {
                seat_code: seat_code.to_owned(),
                seat_id: seat_id.to_owned(),
                account_id: account_id.map(str::to_owned),
                domjudge_username: domjudge_username.map(str::to_owned),
            }
        }
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct AccountEvidence {
        #[diesel(sql_type = Text)]
        domjudge_username: String,
        #[diesel(sql_type = Text)]
        account_id: String,
        #[diesel(sql_type = Text)]
        credential_vault_record_id: String,
        #[diesel(sql_type = BigInt)]
        credential_revision: i64,
        #[diesel(sql_type = Text)]
        record_type: String,
        #[diesel(sql_type = Text)]
        subject_id: String,
        #[diesel(sql_type = Binary)]
        nonce: Vec<u8>,
        #[diesel(sql_type = Binary)]
        ciphertext: Vec<u8>,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct BindingEvidence {
        #[diesel(sql_type = Text)]
        seat_code: String,
        #[diesel(sql_type = Text)]
        device_pk: String,
        #[diesel(sql_type = BigInt)]
        binding_revision: i64,
    }

    impl BindingEvidence {
        fn new(seat_code: &str, device_pk: &str, binding_revision: i64) -> Self {
            Self {
                seat_code: seat_code.to_owned(),
                device_pk: device_pk.to_owned(),
                binding_revision,
            }
        }
    }

    #[derive(QueryableByName)]
    struct CandidateEvidence {
        #[diesel(sql_type = Text)]
        candidate_id: String,
        #[diesel(sql_type = Text)]
        expires_at: String,
        #[diesel(sql_type = BigInt)]
        baseline_configuration_revision: i64,
        #[diesel(sql_type = BigInt)]
        baseline_binding_revision: i64,
        #[diesel(sql_type = Binary)]
        preview_token_hash: Vec<u8>,
        #[diesel(sql_type = Text)]
        payload_vault_record_id: String,
        #[diesel(sql_type = Text)]
        redacted_preview_json: String,
        #[diesel(sql_type = Text)]
        record_type: String,
        #[diesel(sql_type = Text)]
        subject_id: String,
        #[diesel(sql_type = Binary)]
        nonce: Vec<u8>,
        #[diesel(sql_type = Binary)]
        ciphertext: Vec<u8>,
        #[diesel(sql_type = BigInt)]
        ttl_valid: i64,
    }

    #[derive(QueryableByName)]
    struct AuditEvidence {
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action_kind: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = Nullable<Text>)]
        resource_id: Option<String>,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = Nullable<Text>)]
        reason_code: Option<String>,
        #[diesel(sql_type = Text)]
        correlation_id: String,
        #[diesel(sql_type = Text)]
        redacted_detail_json: String,
    }

    #[derive(QueryableByName)]
    struct ExpiryAuditEvidence {
        #[diesel(sql_type = BigInt)]
        count: i64,
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action_kind: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = Nullable<Text>)]
        resource_id: Option<String>,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = Nullable<Text>)]
        reason_code: Option<String>,
        #[diesel(sql_type = Text)]
        correlation_id: String,
        #[diesel(sql_type = Text)]
        redacted_detail_json: String,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct PersistenceSnapshot {
        #[diesel(sql_type = BigInt)]
        candidate_count: i64,
        #[diesel(sql_type = BigInt)]
        vault_count: i64,
        #[diesel(sql_type = BigInt)]
        audit_count: i64,
        #[diesel(sql_type = Nullable<Text>)]
        candidate_id: Option<String>,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the import test fixture failed"))]
        FixtureFailed,
        #[snafu(display("the import candidate could not be created"))]
        CandidateCreationFailed,
        #[snafu(display("an invalid upload was accepted"))]
        InvalidUploadWasAccepted,
        #[snafu(display("an invalid upload reached the vault"))]
        InvalidUploadReachedVault,
        #[snafu(display("an invalid upload wrote data"))]
        InvalidUploadWroteData,
        #[snafu(display("import persistence evidence could not be read"))]
        EvidenceFailed,
        #[snafu(display("the import candidate evidence changed"))]
        CandidateEvidenceChanged,
        #[snafu(display("the preview token hash changed"))]
        PreviewTokenHashChanged,
        #[snafu(display("the staged payload could not be opened"))]
        PayloadOpenFailed,
        #[snafu(display("the staged payload evidence changed"))]
        PayloadEvidenceChanged,
        #[snafu(display("the database contained plaintext or a preview token"))]
        DatabaseLeakedSecret,
        #[snafu(display("the import audit evidence changed"))]
        AuditEvidenceChanged,
        #[snafu(display("a live pending candidate was replaced"))]
        PendingCandidateWasReplaced,
        #[snafu(display("the pending-candidate error changed"))]
        PendingErrorChanged,
        #[snafu(display("a rejected pending upload wrote data"))]
        PendingUploadWroteData,
        #[snafu(display("the expired candidate replacement failed"))]
        ExpiredReplacementFailed,
        #[snafu(display("the expired candidate was not replaced"))]
        ExpiredCandidateWasNotReplaced,
        #[snafu(display("the import expiry audit changed"))]
        ExpiryAuditChanged,
        #[snafu(display("the empty import diff changed"))]
        EmptyDiffChanged,
        #[snafu(display("the import commit failed"))]
        CommitFailed,
        #[snafu(display("the material import commit changed"))]
        MaterialCommitChanged,
        #[snafu(display("the committed credential replacement changed"))]
        CredentialReplacementChanged,
        #[snafu(display("the import commit audit changed"))]
        CommitAuditChanged,
        #[snafu(display("the no-op import commit changed"))]
        NoOpCommitChanged,
        #[snafu(display("the mapping-only import commit changed"))]
        MappingOnlyCommitChanged,
        #[snafu(display("a stale import commit succeeded"))]
        StaleCommitSucceeded,
        #[snafu(display("a stale import commit changed business facts"))]
        StaleCommitChangedBusinessFacts,
        #[snafu(display("an import commit with the wrong token succeeded"))]
        MismatchedTokenSucceeded,
        #[snafu(display("a token mismatch changed the import candidate"))]
        MismatchedTokenChangedCandidate,
        #[snafu(display("an operation on an expired import candidate succeeded"))]
        ExpiredOperationSucceeded,
        #[snafu(display("lazy import expiry behavior changed"))]
        ExpiryOperationChanged,
        #[snafu(display("an operation on an unknown import candidate succeeded"))]
        UnknownOperationSucceeded,
        #[snafu(display("an operation on an unknown import candidate wrote data"))]
        UnknownOperationWroteData,
        #[snafu(display("the import discard failed"))]
        DiscardFailed,
        #[snafu(display("the import discard audit changed"))]
        DiscardAuditChanged,
        #[snafu(display("discard did not recover a wedged import candidate"))]
        WedgedDiscardFailed,
    }
}
