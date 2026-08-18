use crate::{
    application::device::{
        DeviceByHardwareProjection, DeviceId, DeviceLifecycleFacts, DevicePersistenceError,
        DeviceState,
        credentials::{CurrentCredentialConsistencyProjection, DeviceTokenAuthenticationFacts},
    },
    db::Transaction,
};
use diesel::{
    OptionalExtension, QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Binary, Nullable, Text},
};

const fn device_by_hardware_sql() -> &'static str {
    "SELECT device_pk AS device_id, hardware_identity_quality, state FROM devices \
     WHERE machine_hardware_id = ?"
}

const fn device_lifecycle_sql() -> &'static str {
    "SELECT devices.state AS persisted_state, \
     (SELECT COUNT(*) FROM device_tokens \
      WHERE device_pk = devices.device_pk) AS token_count, \
     (SELECT COUNT(*) FROM gateway_certificates \
      WHERE device_pk = devices.device_pk AND status <> 'revoked') \
         AS non_revoked_certificate_count \
     FROM devices WHERE devices.device_pk = ?"
}

const fn current_credential_consistency_sql() -> &'static str {
    "SELECT token_counts.value AS token_count, \
     er.gateway_spki_sha256 AS gateway_spki_sha256, \
     dt.enrollment_request_id AS token_request_id, er.state AS request_state, \
     er.resolved_device_pk AS request_resolved_device_id, \
     er.issuance_audit_event_id AS request_issuance_audit_event_id, \
     active_certificate_counts.value AS active_certificate_count, \
     gc.enrollment_request_id AS active_certificate_request_id, \
     gc.spki_sha256 AS active_certificate_spki_sha256 \
     FROM (SELECT COUNT(*) AS value FROM device_tokens WHERE device_pk = ?) token_counts \
     LEFT JOIN device_tokens dt ON dt.device_pk = ? \
     LEFT JOIN enrollment_requests er \
       ON er.enrollment_request_id = dt.enrollment_request_id \
     CROSS JOIN (SELECT COUNT(*) AS value FROM gateway_certificates \
       WHERE device_pk = ? AND status = 'active') active_certificate_counts \
     LEFT JOIN gateway_certificates gc \
       ON gc.device_pk = ? AND gc.status = 'active'"
}

const fn device_token_authentication_sql() -> &'static str {
    "SELECT dt.device_pk AS device_pk, d.machine_hardware_id AS machine_hardware_id, \
     dt.token_hash AS token_hash FROM device_tokens dt JOIN devices d \
     ON d.device_pk = dt.device_pk WHERE dt.token_hash = ?"
}

pub(crate) fn find_device_by_hardware(
    transaction: &mut Transaction<'_>,
    machine_hardware_id: &str,
) -> Result<Option<DeviceByHardwareProjection>, DevicePersistenceError> {
    let row = diesel::sql_query(device_by_hardware_sql())
        .bind::<Text, _>(machine_hardware_id)
        .get_result::<DeviceByHardwareRow>(transaction.connection())
        .optional()
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    row.map(DeviceByHardwareRow::into_projection).transpose()
}

pub(crate) fn find_device_lifecycle(
    transaction: &mut Transaction<'_>,
    device_id: &str,
) -> Result<Option<DeviceLifecycleFacts>, DevicePersistenceError> {
    let row = diesel::sql_query(device_lifecycle_sql())
        .bind::<Text, _>(device_id)
        .get_result::<PersistedLifecycleFacts>(transaction.connection())
        .optional()
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    row.map(PersistedLifecycleFacts::into_facts).transpose()
}

pub(crate) fn current_credential_consistency(
    transaction: &mut Transaction<'_>,
    device_id: &DeviceId,
) -> Result<CurrentCredentialConsistencyProjection, DevicePersistenceError> {
    let device_id = device_id.as_text();
    let row = diesel::sql_query(current_credential_consistency_sql())
        .bind::<Text, _>(&device_id)
        .bind::<Text, _>(&device_id)
        .bind::<Text, _>(&device_id)
        .bind::<Text, _>(&device_id)
        .get_result::<CurrentCredentialConsistencyRow>(transaction.connection())
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    row.into_projection()
}

pub(crate) fn device_token_authentication_facts(
    transaction: &mut Transaction<'_>,
    token_hash: [u8; 32],
) -> Result<Option<DeviceTokenAuthenticationFacts>, DevicePersistenceError> {
    let row = diesel::sql_query(device_token_authentication_sql())
        .bind::<Binary, _>(token_hash.as_slice())
        .get_result::<DeviceTokenAuthenticationRow>(transaction.connection())
        .optional()
        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
    row.map(DeviceTokenAuthenticationRow::into_facts)
        .transpose()
}

#[derive(QueryableByName)]
struct DeviceByHardwareRow {
    #[diesel(sql_type = Text)]
    device_id: String,
    #[diesel(sql_type = Text)]
    hardware_identity_quality: String,
    #[diesel(sql_type = Text)]
    state: String,
}

impl DeviceByHardwareRow {
    fn into_projection(self) -> Result<DeviceByHardwareProjection, DevicePersistenceError> {
        DeviceByHardwareProjection::from_persisted(
            &self.device_id,
            &self.hardware_identity_quality,
            &self.state,
        )
    }
}

#[derive(QueryableByName)]
struct PersistedLifecycleFacts {
    #[diesel(sql_type = Text)]
    persisted_state: String,
    #[diesel(sql_type = BigInt)]
    token_count: i64,
    #[diesel(sql_type = BigInt)]
    non_revoked_certificate_count: i64,
}

impl PersistedLifecycleFacts {
    fn into_facts(self) -> Result<DeviceLifecycleFacts, DevicePersistenceError> {
        if !matches!(self.token_count, 0 | 1) || self.non_revoked_certificate_count < 0 {
            return Err(DevicePersistenceError::InvalidPersistedFacts);
        }
        let state = DeviceState::from_persisted(&self.persisted_state)
            .ok_or(DevicePersistenceError::InvalidPersistedFacts)?;
        Ok(DeviceLifecycleFacts {
            state,
            token_count: self.token_count,
            non_revoked_certificate_count: self.non_revoked_certificate_count,
        })
    }
}

#[derive(QueryableByName)]
struct CurrentCredentialConsistencyRow {
    #[diesel(sql_type = BigInt)]
    token_count: i64,
    #[diesel(sql_type = Nullable<Binary>)]
    gateway_spki_sha256: Option<Vec<u8>>,
    #[diesel(sql_type = Nullable<Text>)]
    token_request_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    request_state: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    request_resolved_device_id: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    request_issuance_audit_event_id: Option<String>,
    #[diesel(sql_type = BigInt)]
    active_certificate_count: i64,
    #[diesel(sql_type = Nullable<Text>)]
    active_certificate_request_id: Option<String>,
    #[diesel(sql_type = Nullable<Binary>)]
    active_certificate_spki_sha256: Option<Vec<u8>>,
}

impl CurrentCredentialConsistencyRow {
    fn into_projection(
        self,
    ) -> Result<CurrentCredentialConsistencyProjection, DevicePersistenceError> {
        CurrentCredentialConsistencyProjection::from_persisted(
            self.token_count,
            self.gateway_spki_sha256,
            self.token_request_id.as_deref(),
            self.request_state.as_deref(),
            self.request_resolved_device_id.as_deref(),
            self.request_issuance_audit_event_id.as_deref(),
            self.active_certificate_count,
            self.active_certificate_request_id.as_deref(),
            self.active_certificate_spki_sha256,
        )
    }
}

#[derive(QueryableByName)]
struct DeviceTokenAuthenticationRow {
    #[diesel(sql_type = Text)]
    device_pk: String,
    #[diesel(sql_type = Text)]
    machine_hardware_id: String,
    #[diesel(sql_type = Binary)]
    token_hash: Vec<u8>,
}

impl DeviceTokenAuthenticationRow {
    fn into_facts(self) -> Result<DeviceTokenAuthenticationFacts, DevicePersistenceError> {
        DeviceTokenAuthenticationFacts::from_persisted(
            &self.device_pk,
            &self.machine_hardware_id,
            self.token_hash,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use diesel::{QueryableByName, RunQueryDsl, sql_types::Text};
    use uuid::Uuid;

    use crate::{
        application::device::DevicePersistenceError,
        db::{Database, DatabaseConfig},
    };

    use super::{
        CurrentCredentialConsistencyRow, DeviceByHardwareRow, DeviceTokenAuthenticationRow,
        current_credential_consistency_sql, device_by_hardware_sql,
        device_token_authentication_sql,
    };

    const DEVICE_ID: &str = "01900000-0000-7000-8000-000000000001";
    const NON_RFC4122_DEVICE_ID: &str = "01900000-0000-7000-0000-000000000001";
    const MACHINE_ID: &str = "550e8400-e29b-51d4-a716-446655440000";

    #[derive(QueryableByName)]
    struct QueryPlanRow {
        #[diesel(sql_type = Text)]
        detail: String,
    }

    #[tokio::test]
    async fn lifecycle_query_uses_device_and_token_guarding_indexes() {
        let path = std::env::temp_dir().join(format!(
            "natsume-device-query-plan-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|_| panic!("device query-plan database could not be created"));
        let details = database
            .read(|transaction| {
                diesel::sql_query(
                    "EXPLAIN QUERY PLAN \
                     SELECT devices.state, \
                     (SELECT COUNT(*) FROM device_tokens \
                      WHERE device_pk = devices.device_pk), \
                     (SELECT COUNT(*) FROM gateway_certificates \
                      WHERE device_pk = devices.device_pk AND status <> 'revoked') \
                     FROM devices WHERE devices.device_pk = \
                     '01900000-0000-7000-8000-000000000001'",
                )
                .load::<QueryPlanRow>(transaction.connection())
                .map(plan_details)
                .map_err(|_| DevicePersistenceError::PersistenceFailed)
            })
            .await
            .unwrap_or_else(|_| panic!("device query plan could not be read"));

        for required_index in [
            "sqlite_autoindex_devices_1",
            "sqlite_autoindex_device_tokens_1",
        ] {
            assert_uses(&details, required_index);
        }

        drop(database);
        remove_database_artifacts(&path);
    }

    #[tokio::test]
    async fn device_read_models_keep_their_guarding_indexes() {
        let path = std::env::temp_dir().join(format!(
            "natsume-device-query-plan-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|_| panic!("Device query-plan database could not be created"));
        let plans = database
            .read(|transaction| {
                let device =
                    diesel::sql_query(format!("EXPLAIN QUERY PLAN {}", device_by_hardware_sql()))
                        .bind::<Text, _>("00000000-0000-5000-8000-000000000001")
                        .load::<QueryPlanRow>(transaction.connection())
                        .map(plan_details)
                        .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
                let current = diesel::sql_query(format!(
                    "EXPLAIN QUERY PLAN {}",
                    current_credential_consistency_sql()
                ))
                .bind::<Text, _>("01900000-0000-7000-8000-000000000001")
                .bind::<Text, _>("01900000-0000-7000-8000-000000000001")
                .bind::<Text, _>("01900000-0000-7000-8000-000000000001")
                .bind::<Text, _>("01900000-0000-7000-8000-000000000001")
                .load::<QueryPlanRow>(transaction.connection())
                .map(plan_details)
                .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
                let authentication = diesel::sql_query(format!(
                    "EXPLAIN QUERY PLAN {}",
                    device_token_authentication_sql()
                ))
                .bind::<diesel::sql_types::Binary, _>([0_u8; 32].as_slice())
                .load::<QueryPlanRow>(transaction.connection())
                .map(plan_details)
                .map_err(|_| DevicePersistenceError::PersistenceFailed)?;
                Ok::<_, DevicePersistenceError>((device, current, authentication))
            })
            .await
            .unwrap_or_else(|_| panic!("Device query plans could not be read"));

        assert_uses(&plans.0, "sqlite_autoindex_devices_2");
        for required_index in [
            "sqlite_autoindex_device_tokens_1",
            "sqlite_autoindex_enrollment_requests_1",
            "one_active_gateway_certificate",
        ] {
            assert_uses(&plans.1, required_index);
        }
        for required_index in [
            "sqlite_autoindex_device_tokens_3",
            "sqlite_autoindex_devices_1",
        ] {
            assert_uses(&plans.2, required_index);
        }

        drop(database);
        remove_database_artifacts(&path);
    }

    #[test]
    fn invalid_persisted_authentication_device_uuid_fails_at_the_read_boundary() {
        let result = DeviceTokenAuthenticationRow {
            device_pk: MACHINE_ID.to_owned(),
            machine_hardware_id: MACHINE_ID.to_owned(),
            token_hash: vec![0_u8; 32],
        }
        .into_facts();
        assert!(matches!(
            result,
            Err(DevicePersistenceError::InvalidPersistedFacts)
        ));
    }

    #[test]
    fn invalid_persisted_authentication_machine_uuid_fails_at_the_read_boundary() {
        let result = DeviceTokenAuthenticationRow {
            device_pk: DEVICE_ID.to_owned(),
            machine_hardware_id: MACHINE_ID.to_uppercase(),
            token_hash: vec![0_u8; 32],
        }
        .into_facts();
        assert!(matches!(
            result,
            Err(DevicePersistenceError::InvalidPersistedFacts)
        ));
    }

    #[test]
    fn invalid_persisted_authentication_hash_length_fails_at_the_read_boundary() {
        let result = DeviceTokenAuthenticationRow {
            device_pk: DEVICE_ID.to_owned(),
            machine_hardware_id: MACHINE_ID.to_owned(),
            token_hash: vec![0_u8; 31],
        }
        .into_facts();
        assert!(matches!(
            result,
            Err(DevicePersistenceError::InvalidPersistedFacts)
        ));
    }

    #[test]
    fn invalid_persisted_device_state_and_quality_fail_as_device_facts() {
        for (hardware_identity_quality, state) in
            [("excellent", "enrolled"), ("strong", "quarantined")]
        {
            let result = DeviceByHardwareRow {
                device_id: DEVICE_ID.to_owned(),
                hardware_identity_quality: hardware_identity_quality.to_owned(),
                state: state.to_owned(),
            }
            .into_projection();
            assert!(matches!(
                result,
                Err(DevicePersistenceError::InvalidPersistedFacts)
            ));
        }
    }

    #[test]
    fn non_rfc4122_persisted_device_ids_fail_in_every_shared_read_model() {
        let authentication = DeviceTokenAuthenticationRow {
            device_pk: NON_RFC4122_DEVICE_ID.to_owned(),
            machine_hardware_id: MACHINE_ID.to_owned(),
            token_hash: vec![0_u8; 32],
        }
        .into_facts();
        let by_hardware = DeviceByHardwareRow {
            device_id: NON_RFC4122_DEVICE_ID.to_owned(),
            hardware_identity_quality: "strong".to_owned(),
            state: "enrolled".to_owned(),
        }
        .into_projection();
        let current = CurrentCredentialConsistencyRow {
            token_count: 0,
            gateway_spki_sha256: None,
            token_request_id: None,
            request_state: None,
            request_resolved_device_id: Some(NON_RFC4122_DEVICE_ID.to_owned()),
            request_issuance_audit_event_id: None,
            active_certificate_count: 0,
            active_certificate_request_id: None,
            active_certificate_spki_sha256: None,
        }
        .into_projection();

        for result in [
            authentication.map(|_| ()),
            by_hardware.map(|_| ()),
            current.map(|_| ()),
        ] {
            assert!(matches!(
                result,
                Err(DevicePersistenceError::InvalidPersistedFacts)
            ));
        }
    }

    fn plan_details(rows: Vec<QueryPlanRow>) -> Vec<String> {
        rows.into_iter().map(|row| row.detail).collect()
    }

    fn assert_uses(details: &[String], expected: &str) {
        assert!(
            details.iter().any(|detail| detail.contains(expected)),
            "{expected} was absent from query plan: {details:?}"
        );
    }

    fn remove_database_artifacts(path: &Path) {
        for artifact in [
            path.to_path_buf(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = fs::remove_file(artifact);
        }
    }
}
