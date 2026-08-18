use diesel::{
    ExpressionMethods, JoinOnDsl, NullableExpressionMethods, QueryDsl, RunQueryDsl,
    sql_types::BigInt,
};

use crate::{
    application::{
        device::DeviceId,
        import::{CurrentAccountProjection, CurrentSeatProjection, ImportError},
    },
    db::{
        Transaction,
        schema::{account_mappings, accounts, device_bindings, seats},
    },
};

pub(crate) fn read_current_seats(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<CurrentSeatProjection>, ImportError> {
    let rows = seats::table
        .left_join(account_mappings::table.on(account_mappings::seat_id.eq(seats::seat_id)))
        .left_join(accounts::table.on(account_mappings::account_id.eq(accounts::account_id)))
        .left_join(device_bindings::table.on(device_bindings::seat_id.eq(seats::seat_id)))
        .select((
            seats::seat_id,
            seats::seat_code,
            accounts::domjudge_username.nullable(),
            device_bindings::device_pk.nullable(),
        ))
        .order(seats::seat_code)
        .load::<(String, String, Option<String>, Option<String>)>(transaction.connection())
        .map_err(|_| ImportError::PersistenceFailure)?;
    rows.into_iter()
        .map(
            |(seat_id, seat_code, current_domjudge_username, device_id)| {
                let device_id = device_id
                    .map(|device_id| {
                        DeviceId::parse(&device_id).ok_or(ImportError::PersistenceFailure)
                    })
                    .transpose()?;
                Ok(CurrentSeatProjection::new(
                    seat_id,
                    seat_code,
                    current_domjudge_username,
                    device_id,
                ))
            },
        )
        .collect()
}

pub(crate) fn read_current_accounts(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<CurrentAccountProjection>, ImportError> {
    accounts::table
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            accounts::credential_vault_record_id,
            diesel::dsl::sql::<BigInt>("credential_revision"),
        ))
        .order(accounts::domjudge_username)
        .load::<(String, String, String, i64)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(
                    |(
                        account_id,
                        domjudge_username,
                        credential_vault_record_id,
                        credential_revision,
                    )| {
                        CurrentAccountProjection::new(
                            account_id,
                            domjudge_username,
                            credential_vault_record_id,
                            credential_revision,
                        )
                    },
                )
                .collect()
        })
        .map_err(|_| ImportError::PersistenceFailure)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use diesel::{QueryableByName, RunQueryDsl, sql_types::Text};
    use uuid::Uuid;

    use crate::{
        application::import::ImportError,
        db::{Database, DatabaseConfig},
    };

    #[derive(QueryableByName)]
    struct QueryPlanRow {
        #[diesel(sql_type = Text)]
        detail: String,
    }

    type QueryPlans = Vec<(&'static str, Vec<String>)>;

    #[tokio::test]
    async fn import_read_models_use_all_guarding_indexes() {
        let path = std::env::temp_dir().join(format!(
            "natsume-import-query-plan-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|_| panic!("import query-plan database could not be created"));
        let plans = database
            .read(read_query_plans)
            .await
            .unwrap_or_else(|_| panic!("import query plans could not be read"));
        assert_guarding_indexes(&plans);

        drop(database);
        for artifact in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = fs::remove_file(artifact);
        }
    }

    fn assert_guarding_indexes(plans: &QueryPlans) {
        for required_index in [
            "sqlite_autoindex_seats_2",
            "sqlite_autoindex_account_mappings_1",
            "sqlite_autoindex_accounts_1",
            "sqlite_autoindex_device_bindings_1",
        ] {
            assert!(
                plan(plans, "current seats")
                    .iter()
                    .any(|detail| detail.contains(required_index)),
                "{required_index} was absent from current-seat plan: {:?}",
                plan(plans, "current seats")
            );
        }
        assert!(
            plan(plans, "current accounts")
                .iter()
                .any(|detail| detail.contains("sqlite_autoindex_accounts_2")),
            "account username index was absent: {:?}",
            plan(plans, "current accounts")
        );
        for table in ["pending_import_candidate", "revision_counters"] {
            let plan_name = if table == "pending_import_candidate" {
                "pending candidate"
            } else {
                "revision counters"
            };
            assert!(
                plan(plans, plan_name).iter().any(|detail| {
                    detail.contains(table) && detail.contains("USING INTEGER PRIMARY KEY")
                }),
                "{table} singleton primary-key lookup was absent: {:?}",
                plan(plans, plan_name)
            );
        }
        assert!(
            plan(plans, "import payload")
                .iter()
                .any(|detail| { detail.contains("sqlite_autoindex_server_vault_records_2") }),
            "vault record-type/subject index was absent: {:?}",
            plan(plans, "import payload")
        );
    }

    fn plan<'a>(plans: &'a QueryPlans, name: &str) -> &'a Vec<String> {
        plans
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map_or_else(
                || panic!("{name} query plan was absent"),
                |(_, details)| details,
            )
    }

    fn read_query_plans(
        transaction: &mut crate::db::Transaction<'_>,
    ) -> Result<QueryPlans, ImportError> {
        let queries = [
            (
                "current seats",
                "EXPLAIN QUERY PLAN \
                 SELECT seats.seat_id, seats.seat_code, accounts.domjudge_username, \
                        device_bindings.device_pk \
                 FROM seats \
                 LEFT JOIN account_mappings ON account_mappings.seat_id = seats.seat_id \
                 LEFT JOIN accounts ON account_mappings.account_id = accounts.account_id \
                 LEFT JOIN device_bindings ON device_bindings.seat_id = seats.seat_id \
                 ORDER BY seats.seat_code",
            ),
            (
                "current accounts",
                "EXPLAIN QUERY PLAN \
                 SELECT account_id, domjudge_username, credential_vault_record_id, \
                        credential_revision \
                 FROM accounts ORDER BY domjudge_username",
            ),
            (
                "pending candidate",
                "EXPLAIN QUERY PLAN \
                 SELECT candidate_id, expires_at, payload_vault_record_id, \
                        baseline_configuration_revision, baseline_binding_revision, \
                        preview_token_hash, redacted_preview_json \
                 FROM pending_import_candidate WHERE singleton = 1",
            ),
            (
                "revision counters",
                "EXPLAIN QUERY PLAN \
                 SELECT configuration_revision, binding_revision \
                 FROM revision_counters WHERE singleton = 1",
            ),
            (
                "import payload",
                "EXPLAIN QUERY PLAN \
                 SELECT nonce, ciphertext FROM server_vault_records \
                 WHERE vault_record_id = '01900000-0000-7000-8000-000000000001' \
                   AND record_type = 'import_payload' \
                   AND subject_id = '01900000-0000-7000-8000-000000000002'",
            ),
        ];
        queries
            .into_iter()
            .map(|(name, sql)| {
                diesel::sql_query(sql)
                    .load::<QueryPlanRow>(transaction.connection())
                    .map(|rows| {
                        (
                            name,
                            rows.into_iter().map(|row| row.detail).collect::<Vec<_>>(),
                        )
                    })
                    .map_err(|_| ImportError::PersistenceFailure)
            })
            .collect()
    }
}
