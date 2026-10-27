use diesel::{
    ExpressionMethods, OptionalExtension, QueryDsl, RunQueryDsl, dsl::sql, sql_types::BigInt,
};

use crate::{
    component::operator::{OperatorIdentity, SessionFacts},
    db::{PersistenceError, Transaction},
    diesel_schema::{operator_accounts, operator_sessions},
};

pub(in crate::component::operator) fn find_session(
    transaction: &mut Transaction<'_>,
    credential_hash: &[u8; 32],
) -> Result<Option<SessionFacts>, PersistenceError> {
    let row = operator_sessions::table
        .inner_join(operator_accounts::table)
        .select((
            operator_accounts::operator_id,
            operator_accounts::role,
            operator_sessions::expires_at_unix_ms
                .le(sql::<BigInt>("CAST(unixepoch('subsec') * 1000 AS INTEGER)")),
        ))
        .filter(operator_sessions::session_credential_hash.eq(credential_hash.as_slice()))
        .get_result::<(String, String, bool)>(transaction.connection())
        .optional()
        .map_err(PersistenceError::from)?;

    row.map(|(operator_id, role, expired)| {
        let identity = OperatorIdentity::from_persisted(&operator_id, &role)
            .map_err(super::persisted_identity_error)?;
        Ok(SessionFacts { identity, expired })
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
                     SELECT operator_accounts.operator_id, operator_accounts.role, \
                     operator_sessions.expires_at_unix_ms <= \
                     CAST(unixepoch('subsec') * 1000 AS INTEGER) \
                     FROM operator_sessions \
                     INNER JOIN operator_accounts \
                     ON operator_accounts.operator_id = operator_sessions.operator_id \
                     WHERE operator_sessions.session_credential_hash = zeroblob(32)",
                )
                .load::<QueryPlanRow>(transaction.connection())
                .map(|rows| rows.into_iter().map(|row| row.detail).collect::<Vec<_>>())
                .map_err(PersistenceError::from)
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
