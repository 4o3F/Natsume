use diesel::{
    OptionalExtension, QueryableByName, RunQueryDsl,
    sql_types::{BigInt, Binary, Text},
};

use crate::{
    component::operator::{OperatorIdentity, SessionFacts},
    db::{PersistenceError, Transaction},
};

#[derive(QueryableByName)]
struct PersistedSessionFacts {
    #[diesel(sql_type = Text)]
    operator_id: String,
    #[diesel(sql_type = Text)]
    role: String,
    #[diesel(sql_type = BigInt)]
    expired: i64,
}

pub(in crate::component::operator) fn find_session(
    transaction: &mut Transaction<'_>,
    credential_hash: &[u8; 32],
) -> Result<Option<SessionFacts>, PersistenceError> {
    let row = diesel::sql_query(
        "SELECT accounts.operator_id AS operator_id, accounts.role AS role, \
         CASE WHEN sessions.expires_at_unix_ms <= \
              CAST(unixepoch('subsec') * 1000 AS INTEGER) THEN 1 ELSE 0 END AS expired \
         FROM operator_sessions AS sessions \
         INNER JOIN operator_accounts AS accounts ON accounts.operator_id = sessions.operator_id \
         WHERE sessions.session_credential_hash = ?",
    )
    .bind::<Binary, _>(credential_hash.as_slice())
    .get_result::<PersistedSessionFacts>(transaction.connection())
    .optional()
    .map_err(|_| PersistenceError::OperationFailed)?;

    row.map(|row| {
        if !matches!(row.expired, 0 | 1) {
            return Err(PersistenceError::InvalidPersistedData);
        }
        let identity = OperatorIdentity::from_persisted(&row.operator_id, &row.role)
            .map_err(|_| PersistenceError::InvalidPersistedData)?;
        Ok(SessionFacts {
            identity,
            expired: row.expired == 1,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use diesel::{QueryableByName, RunQueryDsl, sql_types::Text};
    use uuid::Uuid;

    use crate::db::{Database, DatabaseConfig, PersistenceError};

    #[derive(QueryableByName)]
    struct QueryPlanRow {
        #[diesel(sql_type = Text)]
        detail: String,
    }

    #[tokio::test]
    async fn session_identity_query_uses_both_primary_key_indexes() {
        let path = std::env::temp_dir().join(format!(
            "natsume-operator-query-plan-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|_| panic!("operator query-plan database could not be created"));
        let details = database
            .read(|transaction| {
                diesel::sql_query(
                    "EXPLAIN QUERY PLAN \
                     SELECT accounts.operator_id, accounts.role, \
                     CASE WHEN sessions.expires_at_unix_ms <= \
                     CAST(unixepoch('subsec') * 1000 AS INTEGER) THEN 1 ELSE 0 END \
                     FROM operator_sessions AS sessions \
                     INNER JOIN operator_accounts AS accounts \
                     ON accounts.operator_id = sessions.operator_id \
                     WHERE sessions.session_credential_hash = zeroblob(32)",
                )
                .load::<QueryPlanRow>(transaction.connection())
                .map(|rows| rows.into_iter().map(|row| row.detail).collect::<Vec<_>>())
                .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|_| panic!("operator query plan could not be read"));

        assert!(
            details
                .iter()
                .any(|detail| detail.contains("sqlite_autoindex_operator_sessions_1")),
            "operator session PK was absent from query plan: {details:?}"
        );
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("sqlite_autoindex_operator_accounts_1")),
            "operator account PK was absent from query plan: {details:?}"
        );

        drop(database);
        for artifact in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.display())),
            PathBuf::from(format!("{}-shm", path.display())),
        ] {
            let _ = fs::remove_file(artifact);
        }
    }
}
