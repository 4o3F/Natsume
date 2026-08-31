use diesel::{ExpressionMethods, JoinOnDsl, NullableExpressionMethods, QueryDsl, RunQueryDsl};

use crate::{
    component::{
        contest::{CurrentAccountProjection, CurrentSeatProjection},
        import::ImportError,
        lifecycle::DeviceId,
    },
    db::Transaction,
    diesel_schema::{account_mappings, accounts, device_bindings, seats},
};

pub(in crate::component::import) fn read_current_seats(
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
            device_bindings::device_id.nullable(),
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

pub(in crate::component::import) fn read_current_accounts(
    transaction: &mut Transaction<'_>,
) -> Result<Vec<CurrentAccountProjection>, ImportError> {
    accounts::table
        .select((
            accounts::account_id,
            accounts::domjudge_username,
            accounts::credential_revision,
        ))
        .order(accounts::domjudge_username)
        .load::<(String, String, i64)>(transaction.connection())
        .map(|rows| {
            rows.into_iter()
                .map(|(account_id, domjudge_username, credential_revision)| {
                    CurrentAccountProjection::new(
                        account_id,
                        domjudge_username,
                        credential_revision,
                    )
                })
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
        component::import::ImportError,
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
            plan(plans, "current seats")
                .iter()
                .any(|detail| detail.contains("device_bindings") && detail.contains("USING INDEX")),
            "the Binding seat index was absent: {:?}",
            plan(plans, "current seats")
        );
        assert!(
            plan(plans, "current accounts")
                .iter()
                .any(|detail| detail.contains("sqlite_autoindex_accounts_2")),
            "account username index was absent: {:?}",
            plan(plans, "current accounts")
        );
        assert!(
            plan(plans, "pending candidate").iter().any(|detail| {
                detail.contains("pending_import_candidate")
                    && detail.contains("USING INTEGER PRIMARY KEY")
            }),
            "candidate singleton lookup was absent: {:?}",
            plan(plans, "pending candidate")
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
                        device_bindings.device_id \
                 FROM seats \
                 LEFT JOIN account_mappings ON account_mappings.seat_id = seats.seat_id \
                 LEFT JOIN accounts ON account_mappings.account_id = accounts.account_id \
                 LEFT JOIN device_bindings ON device_bindings.seat_id = seats.seat_id \
                 ORDER BY seats.seat_code",
            ),
            (
                "current accounts",
                "EXPLAIN QUERY PLAN \
                 SELECT account_id, domjudge_username, credential_revision \
                 FROM accounts ORDER BY domjudge_username",
            ),
            (
                "pending candidate",
                "EXPLAIN QUERY PLAN \
                 SELECT candidate_id, expires_at_unix_ms, preview_token_hash, \
                        fingerprint_version, candidate_fingerprint_sha256, \
                        baseline_fingerprint_sha256, redacted_preview_json \
                 FROM pending_import_candidate WHERE singleton = 1",
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
