#[cfg(test)]
mod orchestration_tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Mutex, MutexGuard},
    };

    use diesel::{
        RunQueryDsl,
        sql_types::{Binary, Text},
    };
    use serde_json::Value;
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        application::device::{
            DeviceConnectionEvictor, DeviceError, DeviceId, DeviceLifecycleAction,
            NoLiveDeviceConnections, disable_device, revoke_device,
        },
        audit::CorrelationId,
        db::{
            Database, DatabaseConfig,
            device::tests::{
                TestLifecycleAudit, TestLifecycleSnapshot, test_latest_lifecycle_audit,
                test_lifecycle_audit_count, test_lifecycle_snapshot, test_seed_lifecycle_device,
            },
            tests::{test_data_version, test_observer},
        },
    };

    const ENROLLED_DEVICE: &str = "01900000-0000-7000-8000-000000000101";
    const DISABLED_DEVICE: &str = "01900000-0000-7000-8000-000000000102";
    const REVOKED_DEVICE: &str = "01900000-0000-7000-8000-000000000103";
    const PARTIAL_DEVICE: &str = "01900000-0000-7000-8000-000000000104";
    const CERTIFICATE_PARTIAL_DEVICE: &str = "01900000-0000-7000-8000-000000000105";

    #[tokio::test]
    async fn successful_and_noop_lifecycle_calls_each_evict_exactly_once() -> Result<(), TestFailure>
    {
        let fixture = TestDatabase::new().await?;
        for id in [ENROLLED_DEVICE, DISABLED_DEVICE] {
            test_seed_lifecycle_device(&fixture.database, id, "enrolled", true, "active")
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
        }

        let revoke_id = device_id(ENROLLED_DEVICE)?;
        let revoke_success = CountingEvictor::default();
        revoke_device(
            &fixture.database,
            &revoke_id,
            correlation_id(),
            &revoke_success,
        )
        .await
        .map_err(|_| TestFailure::LifecycleFailed)?;
        assert_single_eviction(&revoke_success, ENROLLED_DEVICE)?;
        let revoke_noop = CountingEvictor::default();
        revoke_device(
            &fixture.database,
            &revoke_id,
            correlation_id(),
            &revoke_noop,
        )
        .await
        .map_err(|_| TestFailure::LifecycleFailed)?;
        assert_single_eviction(&revoke_noop, ENROLLED_DEVICE)?;

        let disable_id = device_id(DISABLED_DEVICE)?;
        let disable_success = CountingEvictor::default();
        disable_device(
            &fixture.database,
            &disable_id,
            correlation_id(),
            &disable_success,
        )
        .await
        .map_err(|_| TestFailure::LifecycleFailed)?;
        assert_single_eviction(&disable_success, DISABLED_DEVICE)?;
        let disable_noop = CountingEvictor::default();
        disable_device(
            &fixture.database,
            &disable_id,
            correlation_id(),
            &disable_noop,
        )
        .await
        .map_err(|_| TestFailure::LifecycleFailed)?;
        assert_single_eviction(&disable_noop, DISABLED_DEVICE)
    }

    #[tokio::test]
    async fn missing_device_errors_leave_the_database_unchanged_and_never_evict()
    -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        let missing = device_id(ENROLLED_DEVICE)?;
        let mut observer = test_observer(&fixture.path).map_err(|_| TestFailure::EvidenceFailed)?;
        let version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::EvidenceFailed)?;
        let revoke_evictor = CountingEvictor::default();
        let revoke_result = revoke_device(
            &fixture.database,
            &missing,
            correlation_id(),
            &revoke_evictor,
        )
        .await;
        let disable_evictor = CountingEvictor::default();
        let disable_result = disable_device(
            &fixture.database,
            &missing,
            correlation_id(),
            &disable_evictor,
        )
        .await;
        let version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::EvidenceFailed)?;
        if revoke_result != Err(DeviceError::DeviceNotFound)
            || disable_result != Err(DeviceError::DeviceNotFound)
            || !revoke_evictor.evictions().is_empty()
            || !disable_evictor.evictions().is_empty()
            || version_after != version_before
        {
            return Err(TestFailure::FailedLifecycleEvictedConnection);
        }
        Ok(())
    }

    #[tokio::test]
    async fn revoke_converges_then_records_a_business_zero_write_noop() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        seed_additional_certificate(&fixture.database, ENROLLED_DEVICE, "retired", 1).await?;
        seed_additional_certificate(&fixture.database, ENROLLED_DEVICE, "expired", 2).await?;
        let device_id = device_id(ENROLLED_DEVICE)?;

        apply(&fixture.database, &device_id, DeviceLifecycleAction::Revoke).await?;
        let applied = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let succeeded = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        verify_revoke_effect(&applied, 3)?;
        verify_audit(
            &succeeded,
            "revoke_device",
            "succeeded",
            "operator_requested",
            r#"{"resulting_state":"revoked","removed_token_count":1,"revoked_certificate_count":3}"#,
        )?;
        verify_audit_is_allowlisted(&succeeded, ENROLLED_DEVICE)?;

        let mut observer = test_observer(&fixture.path).map_err(|_| TestFailure::EvidenceFailed)?;
        let version_before =
            test_data_version(&mut observer).map_err(|_| TestFailure::EvidenceFailed)?;
        apply(&fixture.database, &device_id, DeviceLifecycleAction::Revoke).await?;
        let after_noop = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let version_after =
            test_data_version(&mut observer).map_err(|_| TestFailure::EvidenceFailed)?;
        let noop = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let audit_count = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if applied != after_noop || audit_count != 2 || version_after == version_before {
            return Err(TestFailure::NoopChangedBusinessFacts);
        }
        verify_audit(
            &noop,
            "revoke_device",
            "noop",
            "target_already_satisfied",
            r#"{"resulting_state":"revoked","removed_token_count":0,"revoked_certificate_count":0}"#,
        )
    }

    #[tokio::test]
    async fn disable_is_repeat_safe_and_never_weakens_revoked() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        for (id, state, token, certificate) in [
            (ENROLLED_DEVICE, "enrolled", true, "active"),
            (REVOKED_DEVICE, "revoked", false, "revoked"),
        ] {
            test_seed_lifecycle_device(&fixture.database, id, state, token, certificate)
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
        }

        let enrolled = device_id(ENROLLED_DEVICE)?;
        apply(&fixture.database, &enrolled, DeviceLifecycleAction::Disable).await?;
        let disabled_once = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let disabled_succeeded = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        apply(&fixture.database, &enrolled, DeviceLifecycleAction::Disable).await?;
        let disabled_twice = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let disabled_noop = test_latest_lifecycle_audit(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let disable_audit_count = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if disabled_once.state != "disabled"
            || disabled_once.token_count != 1
            || disabled_once.certificate_statuses != ["active"]
            || disabled_once.binding_revision != 43
            || disabled_once.configuration_revision != 41
            || disabled_once.global_binding_revision != 43
            || disabled_once.command_count != 0
            || disabled_once != disabled_twice
            || disable_audit_count != 2
        {
            return Err(TestFailure::DisableTransitionChanged);
        }
        verify_audit(
            &disabled_succeeded,
            "disable_device",
            "succeeded",
            "operator_requested",
            r#"{"resulting_state":"disabled","removed_token_count":0,"revoked_certificate_count":0}"#,
        )?;
        verify_audit(
            &disabled_noop,
            "disable_device",
            "noop",
            "target_already_satisfied",
            r#"{"resulting_state":"disabled","removed_token_count":0,"revoked_certificate_count":0}"#,
        )?;

        let revoked = device_id(REVOKED_DEVICE)?;
        let revoked_before = test_lifecycle_snapshot(&fixture.database, REVOKED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        apply(&fixture.database, &revoked, DeviceLifecycleAction::Disable).await?;
        let revoked_after = test_lifecycle_snapshot(&fixture.database, REVOKED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let revoked_noop = test_latest_lifecycle_audit(&fixture.database, REVOKED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if revoked_before != revoked_after
            || revoked_after.state != "revoked"
            || revoked_after.token_count != 0
            || revoked_after.certificate_statuses != ["revoked"]
            || revoked_after.binding_revision != 43
            || revoked_after.configuration_revision != 41
            || revoked_after.global_binding_revision != 43
            || revoked_after.command_count != 0
        {
            return Err(TestFailure::StrongerStateWasWeakened);
        }
        verify_audit(
            &revoked_noop,
            "disable_device",
            "noop",
            "target_already_satisfied",
            r#"{"resulting_state":"revoked","removed_token_count":0,"revoked_certificate_count":0}"#,
        )
    }

    #[tokio::test]
    async fn revoke_converges_from_disabled_and_each_partial_target() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        for (id, state, token, certificate) in [
            (DISABLED_DEVICE, "disabled", true, "active"),
            (PARTIAL_DEVICE, "revoked", true, "revoked"),
            (CERTIFICATE_PARTIAL_DEVICE, "revoked", false, "active"),
        ] {
            test_seed_lifecycle_device(&fixture.database, id, state, token, certificate)
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
        }
        for (id, detail) in [
            (
                DISABLED_DEVICE,
                r#"{"resulting_state":"revoked","removed_token_count":1,"revoked_certificate_count":1}"#,
            ),
            (
                PARTIAL_DEVICE,
                r#"{"resulting_state":"revoked","removed_token_count":1,"revoked_certificate_count":0}"#,
            ),
            (
                CERTIFICATE_PARTIAL_DEVICE,
                r#"{"resulting_state":"revoked","removed_token_count":0,"revoked_certificate_count":1}"#,
            ),
        ] {
            let parsed = device_id(id)?;
            apply(&fixture.database, &parsed, DeviceLifecycleAction::Revoke).await?;
            let snapshot = test_lifecycle_snapshot(&fixture.database, id)
                .await
                .map_err(|_| TestFailure::EvidenceFailed)?;
            let audit = test_latest_lifecycle_audit(&fixture.database, id)
                .await
                .map_err(|_| TestFailure::EvidenceFailed)?;
            verify_revoke_effect(&snapshot, 1)?;
            if audit.result != "succeeded" {
                return Err(TestFailure::PartialRevokeWasReportedAsNoop);
            }
            if audit.detail != detail {
                return Err(TestFailure::AuditChanged);
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_audit_insert_rolls_back_the_full_revoke_effect() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        fixture
            .database
            .test_write(|connection| {
                diesel::sql_query(
                    "CREATE TRIGGER fail_lifecycle_audit BEFORE INSERT ON audit_events \
                     WHEN NEW.action_kind = 'revoke_device' \
                     BEGIN SELECT RAISE(ABORT, 'injected lifecycle audit failure'); END",
                )
                .execute(connection)
                .map(|_| ())
                .map_err(|_| DeviceError::PersistenceFailed)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)?;
        let before = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let before_audits = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let evictor = CountingEvictor::default();
        let result = revoke_device(
            &fixture.database,
            &device_id(ENROLLED_DEVICE)?,
            correlation_id(),
            &evictor,
        )
        .await;
        let after = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let after_audits = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if result != Err(DeviceError::PersistenceFailed)
            || after != before
            || after_audits != before_audits
            || !evictor.evictions().is_empty()
        {
            return Err(TestFailure::AuditFailureDidNotRollBack);
        }
        Ok(())
    }

    #[tokio::test]
    async fn token_and_certificate_failures_roll_back_device_state_and_audit()
    -> Result<(), TestFailure> {
        for trigger in [
            "CREATE TRIGGER fail_lifecycle_token_delete BEFORE DELETE ON device_tokens \
         BEGIN SELECT RAISE(ABORT, 'injected token delete failure'); END",
            "CREATE TRIGGER fail_lifecycle_certificate_update \
         BEFORE UPDATE OF status ON gateway_certificates \
         BEGIN SELECT RAISE(ABORT, 'injected certificate update failure'); END",
        ] {
            assert_lifecycle_mutation_failure_rolls_back(trigger).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn device_state_cas_conflict_rolls_back_the_lifecycle_audit() -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        fixture
            .database
            .test_write(|connection| {
                diesel::sql_query(
                    "CREATE TRIGGER ignore_device_state_cas BEFORE UPDATE OF state ON devices \
                 BEGIN SELECT RAISE(IGNORE); END",
                )
                .execute(connection)
                .map(|_| ())
                .map_err(|_| DeviceError::PersistenceFailed)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)?;
        let before = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let audits_before = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;

        let evictor = CountingEvictor::default();
        let result = revoke_device(
            &fixture.database,
            &device_id(ENROLLED_DEVICE)?,
            correlation_id(),
            &evictor,
        )
        .await;
        let after = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let audits_after = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if result != Err(DeviceError::PersistenceFailed)
            || after != before
            || audits_after != audits_before
            || !evictor.evictions().is_empty()
        {
            return Err(TestFailure::MutationFailureDidNotRollBack);
        }
        Ok(())
    }

    async fn assert_lifecycle_mutation_failure_rolls_back(
        trigger: &'static str,
    ) -> Result<(), TestFailure> {
        let fixture = TestDatabase::new().await?;
        test_seed_lifecycle_device(
            &fixture.database,
            ENROLLED_DEVICE,
            "enrolled",
            true,
            "active",
        )
        .await
        .map_err(|_| TestFailure::FixtureFailed)?;
        fixture
            .database
            .test_write(move |connection| {
                diesel::sql_query(trigger)
                    .execute(connection)
                    .map(|_| ())
                    .map_err(|_| DeviceError::PersistenceFailed)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)?;
        let before = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let audits_before = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;

        let evictor = CountingEvictor::default();
        let result = revoke_device(
            &fixture.database,
            &device_id(ENROLLED_DEVICE)?,
            correlation_id(),
            &evictor,
        )
        .await;
        let after = test_lifecycle_snapshot(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        let audits_after = test_lifecycle_audit_count(&fixture.database, ENROLLED_DEVICE)
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?;
        if result != Err(DeviceError::PersistenceFailed)
            || after != before
            || audits_after != audits_before
            || !evictor.evictions().is_empty()
        {
            return Err(TestFailure::MutationFailureDidNotRollBack);
        }
        Ok(())
    }

    async fn seed_additional_certificate(
        database: &Database,
        device_id: &str,
        status: &'static str,
        marker: u8,
    ) -> Result<(), TestFailure> {
        let device_id = device_id.to_owned();
        database
            .test_write(move |connection| {
                let enrollment_id = format!("enrollment-{status}-{device_id}");
                let hardware_id = format!("hardware-secret-{device_id}");
                let digest = [marker; 32];
                diesel::sql_query(
                    "INSERT INTO enrollment_requests \
                 (enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
                  gateway_csr_der, gateway_spki_sha256, client_version, protocol_version, \
                  source_ip, state, created_at) \
                 VALUES (?, ?, 'strong', x'01', ?, 'test-client', 1, '192.0.2.1', 'expired', \
                         '2026-08-08T00:00:00.000Z')",
                )
                .bind::<Text, _>(&enrollment_id)
                .bind::<Text, _>(&hardware_id)
                .bind::<Binary, _>(digest.as_slice())
                .execute(connection)
                .map_err(|_| DeviceError::PersistenceFailed)?;
                diesel::sql_query(
                    "INSERT INTO gateway_certificates \
                 (certificate_id, device_pk, enrollment_request_id, serial, spki_sha256, \
                  not_after, status) \
                 VALUES (?, ?, ?, ?, ?, '2027-08-08T00:00:00.000Z', ?)",
                )
                .bind::<Text, _>(format!("certificate-{status}-{device_id}"))
                .bind::<Text, _>(&device_id)
                .bind::<Text, _>(&enrollment_id)
                .bind::<Text, _>(format!("certificate-serial-{status}-{device_id}"))
                .bind::<Binary, _>(digest.as_slice())
                .bind::<Text, _>(status)
                .execute(connection)
                .map(|_| ())
                .map_err(|_| DeviceError::PersistenceFailed)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)
    }

    fn verify_revoke_effect(
        snapshot: &TestLifecycleSnapshot,
        expected_certificate_count: usize,
    ) -> Result<(), TestFailure> {
        if snapshot.state != "revoked"
            || snapshot.token_count != 0
            || snapshot.certificate_statuses.len() != expected_certificate_count
            || snapshot
                .certificate_statuses
                .iter()
                .any(|status| status != "revoked")
            || snapshot.binding_revision != 43
            || snapshot.configuration_revision != 41
            || snapshot.global_binding_revision != 43
            || snapshot.command_count != 0
        {
            return Err(TestFailure::RevokeEffectChanged);
        }
        Ok(())
    }

    fn verify_audit(
        audit: &TestLifecycleAudit,
        action: &str,
        result: &str,
        reason: &str,
        detail: &str,
    ) -> Result<(), TestFailure> {
        let parsed: Value =
            serde_json::from_str(&audit.detail).map_err(|_| TestFailure::AuditChanged)?;
        let object = parsed.as_object().ok_or(TestFailure::AuditChanged)?;
        if audit.actor != "operator:self"
            || audit.action != action
            || audit.resource_type != "device"
            || audit.result != result
            || audit.reason != reason
            || audit.detail != detail
            || object.len() != 3
            || !object.contains_key("resulting_state")
            || !object.contains_key("removed_token_count")
            || !object.contains_key("revoked_certificate_count")
        {
            return Err(TestFailure::AuditChanged);
        }
        Ok(())
    }

    fn verify_audit_is_allowlisted(
        audit: &TestLifecycleAudit,
        device_id: &str,
    ) -> Result<(), TestFailure> {
        for forbidden in [
            format!("hardware-secret-{device_id}"),
            format!("certificate-serial-secret-{device_id}"),
            "certificate-material-secret-canary".to_owned(),
            "token-hash-secret-canary-".to_owned(),
            "spki-hash-secret-canary-".to_owned(),
            "spki-secret-canary-".to_owned(),
            "token_hash".to_owned(),
            "machine_hardware_id".to_owned(),
            "serial".to_owned(),
        ] {
            if audit.complete_row.contains(&forbidden) || audit.detail.contains(&forbidden) {
                return Err(TestFailure::AuditLeakedForbiddenEvidence);
            }
        }
        if audit.resource_id != device_id {
            return Err(TestFailure::AuditChanged);
        }
        Ok(())
    }

    fn device_id(value: &str) -> Result<DeviceId, TestFailure> {
        DeviceId::parse(value).ok_or(TestFailure::FixtureFailed)
    }

    async fn apply(
        database: &Database,
        device_id: &DeviceId,
        action: DeviceLifecycleAction,
    ) -> Result<(), TestFailure> {
        match action {
            DeviceLifecycleAction::Revoke => {
                revoke_device(
                    database,
                    device_id,
                    correlation_id(),
                    &NoLiveDeviceConnections,
                )
                .await
            }
            DeviceLifecycleAction::Disable => {
                disable_device(
                    database,
                    device_id,
                    correlation_id(),
                    &NoLiveDeviceConnections,
                )
                .await
            }
        }
        .map_err(|_| TestFailure::LifecycleFailed)
    }

    fn assert_single_eviction(
        evictor: &CountingEvictor,
        expected_device_id: &str,
    ) -> Result<(), TestFailure> {
        if evictor.evictions() != [expected_device_id] {
            return Err(TestFailure::LifecycleEvictionChanged);
        }
        Ok(())
    }

    #[derive(Default)]
    struct CountingEvictor {
        device_ids: Mutex<Vec<String>>,
    }

    impl CountingEvictor {
        fn evictions(&self) -> Vec<String> {
            self.lock_device_ids().clone()
        }

        fn lock_device_ids(&self) -> MutexGuard<'_, Vec<String>> {
            self.device_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl DeviceConnectionEvictor for CountingEvictor {
        fn evict_device_connection(&self, device_pk: &str) -> bool {
            self.lock_device_ids().push(device_pk.to_owned());
            true
        }
    }

    fn correlation_id() -> CorrelationId {
        CorrelationId::from_uuid(Uuid::now_v7())
    }

    struct TestDatabase {
        database: Database,
        path: PathBuf,
    }

    impl TestDatabase {
        async fn new() -> Result<Self, TestFailure> {
            let path = std::env::temp_dir().join(format!(
                "natsume-contest-lifecycle-test-{}.sqlite3",
                Uuid::now_v7()
            ));
            let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
                .await
                .map_err(|_| TestFailure::FixtureFailed)?;
            Ok(Self { database, path })
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _database_result = fs::remove_file(&self.path);
            let _wal_result = fs::remove_file(format!("{}-wal", self.path.display()));
            let _shm_result = fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the lifecycle database fixture failed"))]
        FixtureFailed,
        #[snafu(display("the lifecycle operation failed"))]
        LifecycleFailed,
        #[snafu(display("successful lifecycle connection eviction changed"))]
        LifecycleEvictionChanged,
        #[snafu(display("a failed lifecycle operation evicted a connection"))]
        FailedLifecycleEvictedConnection,
        #[snafu(display("lifecycle persistence evidence could not be read"))]
        EvidenceFailed,
        #[snafu(display("the revoke effect changed"))]
        RevokeEffectChanged,
        #[snafu(display("the repeat revoke changed business facts"))]
        NoopChangedBusinessFacts,
        #[snafu(display("the disable transition changed"))]
        DisableTransitionChanged,
        #[snafu(display("the stronger Device state was weakened"))]
        StrongerStateWasWeakened,
        #[snafu(display("a partial revoke was reported as a no-op"))]
        PartialRevokeWasReportedAsNoop,
        #[snafu(display("the lifecycle audit envelope changed"))]
        AuditChanged,
        #[snafu(display("forbidden lifecycle evidence escaped into audit"))]
        AuditLeakedForbiddenEvidence,
        #[snafu(display("an audit failure did not roll back lifecycle state"))]
        AuditFailureDidNotRollBack,
        #[snafu(display(
            "a token or certificate failure did not roll back lifecycle state and audit"
        ))]
        MutationFailureDidNotRollBack,
    }
}
