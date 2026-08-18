use diesel::{OptionalExtension, RunQueryDsl, sql_types::Text};
use uuid::Uuid;

use crate::{
    application::device::enrollment::{
        EnrollmentDecisionProjection, EnrollmentError, EnrollmentRequestSummary,
        LatestEnrollmentRequestProjection, LiveEnrollmentRequestProjection,
    },
    db::Transaction,
};

use super::row::{
    CountRow, EnrollmentDecisionRow, EnrollmentRequestSummaryRow, LatestEnrollmentRequestRow,
    LiveEnrollmentRequestRow,
};

const fn latest_request_for_hardware_sql() -> &'static str {
    "SELECT state FROM enrollment_requests WHERE machine_hardware_id = ? \
     ORDER BY rowid DESC LIMIT 1"
}

const fn live_requests_for_hardware_sql() -> &'static str {
    "SELECT enrollment_request_id, gateway_spki_sha256, state, resolution, \
     resolved_device_pk AS resolved_device_id FROM enrollment_requests \
     WHERE machine_hardware_id = ? AND state IN ('pending', 'approved') ORDER BY rowid"
}

pub(crate) fn latest_request_for_hardware(
    transaction: &mut Transaction<'_>,
    machine_hardware_id: &str,
) -> Result<Option<LatestEnrollmentRequestProjection>, EnrollmentError> {
    let row = diesel::sql_query(latest_request_for_hardware_sql())
        .bind::<Text, _>(machine_hardware_id)
        .get_result::<LatestEnrollmentRequestRow>(transaction.connection())
        .optional()
        .map_err(|_| EnrollmentError::PersistenceFailed)?;
    row.map(LatestEnrollmentRequestRow::into_projection)
        .transpose()
}

pub(crate) fn live_requests_for_hardware(
    transaction: &mut Transaction<'_>,
    machine_hardware_id: &str,
) -> Result<Vec<LiveEnrollmentRequestProjection>, EnrollmentError> {
    let rows = diesel::sql_query(live_requests_for_hardware_sql())
        .bind::<Text, _>(machine_hardware_id)
        .load::<LiveEnrollmentRequestRow>(transaction.connection())
        .map_err(|_| EnrollmentError::PersistenceFailed)?;
    rows.into_iter()
        .map(LiveEnrollmentRequestRow::into_projection)
        .collect()
}

pub(crate) fn live_request_count(
    transaction: &mut Transaction<'_>,
) -> Result<i64, EnrollmentError> {
    diesel::sql_query(
        "SELECT COUNT(*) AS value FROM enrollment_requests \
         WHERE state IN ('pending', 'approved')",
    )
    .get_result::<CountRow>(transaction.connection())
    .map(|row| row.value)
    .map_err(|_| EnrollmentError::PersistenceFailed)
}

pub(crate) fn request_for_decision(
    transaction: &mut Transaction<'_>,
    request_id: Uuid,
) -> Result<Option<EnrollmentDecisionProjection>, EnrollmentError> {
    let row = diesel::sql_query(
        "SELECT state, resolution, resolved_device_pk AS resolved_device_id, \
         issuance_audit_event_id FROM enrollment_requests WHERE enrollment_request_id = ?",
    )
    .bind::<Text, _>(request_id.to_string())
    .get_result::<EnrollmentDecisionRow>(transaction.connection())
    .optional()
    .map_err(|_| EnrollmentError::PersistenceFailed)?;
    row.map(EnrollmentDecisionRow::into_projection).transpose()
}

pub(crate) fn list_live_requests(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<EnrollmentRequestSummary>, EnrollmentError> {
    let rows = diesel::sql_query(
        "SELECT enrollment_request_id, machine_hardware_id, hardware_identity_quality, \
         gateway_spki_sha256, client_version, protocol_version, state, resolution, \
         resolved_device_pk AS resolved_device_id, created_at, source_ip \
         FROM enrollment_requests WHERE state IN ('pending', 'approved') \
         ORDER BY created_at, enrollment_request_id",
    )
    .load::<EnrollmentRequestSummaryRow>(transaction.connection())
    .map_err(|_| EnrollmentError::PersistenceFailed)?;
    rows.into_iter()
        .map(EnrollmentRequestSummaryRow::into_facts)
        .collect()
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
        application::device::enrollment::EnrollmentError,
        db::{Database, DatabaseConfig},
    };

    use super::{latest_request_for_hardware_sql, live_requests_for_hardware_sql};

    #[derive(QueryableByName)]
    struct QueryPlanRow {
        #[diesel(sql_type = Text)]
        detail: String,
    }

    #[tokio::test]
    async fn enrollment_read_models_keep_their_guarding_indexes() {
        let path = std::env::temp_dir().join(format!(
            "natsume-enrollment-query-plan-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|_| panic!("Enrollment query-plan database could not be created"));
        let plans = database
            .read(|transaction| {
                let latest = diesel::sql_query(format!(
                    "EXPLAIN QUERY PLAN {}",
                    latest_request_for_hardware_sql()
                ))
                .bind::<Text, _>("00000000-0000-5000-8000-000000000001")
                .load::<QueryPlanRow>(transaction.connection())
                .map(plan_details)
                .map_err(|_| EnrollmentError::PersistenceFailed)?;
                let live = diesel::sql_query(format!(
                    "EXPLAIN QUERY PLAN {}",
                    live_requests_for_hardware_sql()
                ))
                .bind::<Text, _>("00000000-0000-5000-8000-000000000001")
                .load::<QueryPlanRow>(transaction.connection())
                .map(plan_details)
                .map_err(|_| EnrollmentError::PersistenceFailed)?;
                Ok::<_, EnrollmentError>((latest, live))
            })
            .await
            .unwrap_or_else(|_| panic!("Enrollment query plans could not be read"));

        assert_uses(&plans.0, "SCAN enrollment_requests");
        assert_uses(&plans.1, "one_live_enrollment_per_machine_and_gateway_spki");

        drop(database);
        remove_database_artifacts(&path);
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
