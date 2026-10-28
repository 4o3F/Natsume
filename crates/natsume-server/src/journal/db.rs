use std::path::Path;

use diesel::{
    ExpressionMethods, QueryDsl, RunQueryDsl,
    connection::SimpleConnection,
    r2d2::CustomizeConnection,
    sql_types::{Integer, Text},
    sqlite::SqliteConnection,
};
use diesel_migrations::{EmbeddedMigrations, embed_migrations};

use crate::{
    db::{Database, DatabaseConfig, DatabaseError, PersistenceError, TransactionError},
    journal_schema::journal_events as events,
};

/// Maximum IDs deleted per short journal-pruning transaction.
pub(super) const PRUNE_CHUNK: u16 = 5000;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("diesel/journal/migrations");

#[derive(Clone)]
pub(super) struct JournalDatabase(Database);

#[derive(Debug, Clone, PartialEq, Eq, diesel::Insertable, diesel::Queryable)]
#[diesel(table_name = events)]
pub(super) struct StoredEvent {
    pub(super) occurred_at_ms: i64,
    pub(super) boot_id: String,
    pub(super) kind: String,
    pub(super) category: String,
    pub(super) severity: String,
    pub(super) device_id: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) actor_operator_id: Option<String>,
    pub(super) request_id: Option<String>,
    pub(super) operation_id: Option<String>,
    pub(super) trace_id: Option<String>,
    pub(super) seat_code: Option<String>,
    pub(super) details: String,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Prune {
    OlderThan(i64),
    RowsBeyond(i64),
    OldestBatch,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Maintenance {
    pub(super) busy: i32,
    pub(super) wal_pages: i32,
    pub(super) checkpointed_pages: i32,
    pub(super) page_count: i64,
}

impl JournalDatabase {
    pub(super) async fn open(path: &Path) -> Result<Self, DatabaseError> {
        if !DatabaseConfig::new(path, true).is_valid() {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let file_path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file_path)
                .map(drop)
                .map_err(|source| DatabaseError::File { source })
        })
        .await
        .map_err(|source| DatabaseError::Executor {
            source: source.into(),
        })??;
        Database::connect_with(
            &DatabaseConfig::new(path, true),
            MIGRATIONS,
            3,
            true,
            Box::new(JournalConnectionCustomizer),
        )
        .await
        .map(Self)
    }

    pub(super) async fn insert_batch(
        &self,
        batch: &[StoredEvent],
    ) -> Result<usize, PersistenceError> {
        let batch = batch.to_owned();
        self.0
            .write(move |transaction| {
                diesel::insert_into(events::table)
                    .values(&batch)
                    .execute(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "read API in S6"))]
    pub(super) async fn read_cursor(
        &self,
        before: Option<i64>,
        after: Option<i64>,
        limit: u16,
    ) -> Result<Vec<(i64, StoredEvent)>, PersistenceError> {
        self.0
            .read(move |transaction| {
                let mut query = events::table
                    .select((
                        events::id,
                        (
                            events::occurred_at_ms,
                            events::boot_id,
                            events::kind,
                            events::category,
                            events::severity,
                            events::device_id,
                            events::session_id,
                            events::actor_operator_id,
                            events::request_id,
                            events::operation_id,
                            events::trace_id,
                            events::seat_code,
                            events::details,
                        ),
                    ))
                    .into_boxed();
                if let Some(before) = before {
                    query = query.filter(events::id.lt(before));
                }
                if let Some(after) = after {
                    query = query.filter(events::id.gt(after));
                }
                query
                    .order(events::id.desc())
                    .limit(i64::from(limit.min(1000)))
                    .load::<(i64, StoredEvent)>(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(super) async fn prune(&self, policy: Prune) -> Result<usize, PersistenceError> {
        self.0
            .write(move |transaction| {
                let mut query = events::table
                    .select(events::id)
                    .order(events::id)
                    .into_boxed();
                let limit = match policy {
                    Prune::OlderThan(cutoff) => {
                        query = query.filter(events::occurred_at_ms.lt(cutoff));
                        i64::from(PRUNE_CHUNK)
                    }
                    Prune::RowsBeyond(max_rows) => {
                        let count = events::table
                            .select(diesel::dsl::count_star())
                            .get_result::<i64>(transaction.connection())?;
                        (count - max_rows.max(0)).clamp(0, i64::from(PRUNE_CHUNK))
                    }
                    Prune::OldestBatch => i64::from(PRUNE_CHUNK),
                };
                let ids = query.limit(limit).load::<i64>(transaction.connection())?;
                diesel::delete(events::table.filter(events::id.eq_any(ids)))
                    .execute(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(super) async fn maintenance(
        &self,
        truncate: bool,
    ) -> Result<Maintenance, PersistenceError> {
        let (busy, wal_pages, checkpointed_pages, page_count) =
            self.0.journal_maintenance(truncate).await?;
        if busy != 0 || checkpointed_pages < wal_pages {
            tracing::warn!(
                busy,
                wal_pages,
                checkpointed_pages,
                "journal checkpoint incomplete"
            );
        }
        Ok(Maintenance {
            busy,
            wal_pages,
            checkpointed_pages,
            page_count,
        })
    }
}

#[derive(Debug)]
struct JournalConnectionCustomizer;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for JournalConnectionCustomizer {
    fn on_acquire(&self, connection: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        // SQLite pragmas configure connections and file headers; Diesel has no typed equivalent.
        connection.batch_execute("PRAGMA auto_vacuum = INCREMENTAL; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")
            .map_err(diesel::r2d2::Error::QueryError)?;
        let values = (|| -> diesel::QueryResult<_> {
            Ok((
                diesel::dsl::sql::<Integer>("PRAGMA auto_vacuum").get_result::<i32>(connection)?,
                diesel::dsl::sql::<Text>("PRAGMA journal_mode").get_result::<String>(connection)?,
                diesel::dsl::sql::<Integer>("PRAGMA synchronous").get_result::<i32>(connection)?,
                diesel::dsl::sql::<Integer>("PRAGMA busy_timeout").get_result::<i32>(connection)?,
            ))
        })()
        .map_err(diesel::r2d2::Error::QueryError)?;
        if values != (2, "wal".to_owned(), 1, 5000) {
            return Err(diesel::r2d2::Error::QueryError(
                diesel::result::Error::NotFound,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::test_error::TestError;

    #[test]
    fn embedded_journal_migration_versions_are_frozen() {
        let migrations =
            diesel::migration::MigrationSource::<diesel::sqlite::Sqlite>::migrations(&MIGRATIONS)
                .unwrap_or_else(|error| panic!("embedded journal migrations unavailable: {error}"));
        let versions = migrations
            .iter()
            .map(|migration| migration.name().version().to_string())
            .collect::<Vec<_>>();
        assert_eq!(versions, ["00000000000001"]);
    }

    pub(in crate::journal) fn fast_busy_timeout(
        database: &JournalDatabase,
    ) -> Result<(), crate::test_error::TestError> {
        for mut connection in database.0.test_exhaust_pool() {
            connection.batch_execute("PRAGMA busy_timeout = 0")?;
        }
        Ok(())
    }

    fn event(at: i64) -> StoredEvent {
        StoredEvent {
            occurred_at_ms: at,
            boot_id: uuid::Uuid::nil().to_string(),
            kind: "server_started".into(),
            category: "journal".into(),
            severity: "info".into(),
            device_id: None,
            session_id: None,
            actor_operator_id: None,
            request_id: None,
            operation_id: None,
            trace_id: None,
            seat_code: None,
            details: "{\"kind\":\"server_started\",\"v\":1}".into(),
        }
    }

    #[tokio::test]
    async fn empty_file_migrates_and_reopens_with_incremental_vacuum() -> Result<(), TestError> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("journal.db");
        std::fs::File::create(&path)?;
        let database = JournalDatabase::open(&path).await?;
        assert_eq!(database.0.pool_size_for_test(), 3);
        assert_eq!(
            database
                .insert_batch(&[event(20), event(10), event(30)])
                .await?,
            3
        );
        let rows = database.read_cursor(None, None, 100).await?;
        assert_eq!(
            rows.iter().map(|row| row.0).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
        assert_eq!(database.read_cursor(Some(3), Some(1), 100).await?[0].0, 2);
        assert_eq!(database.prune(Prune::OlderThan(20)).await?, 1);
        assert_eq!(database.prune(Prune::RowsBeyond(1)).await?, 1);
        assert_eq!(database.read_cursor(None, None, 100).await?[0].0, 3);
        let maintenance = database.maintenance(false).await?;
        assert_eq!(maintenance.busy, 0);
        assert!(maintenance.page_count > 0);
        assert_eq!(database.prune(Prune::OldestBatch).await?, 1);
        assert_eq!(database.maintenance(true).await?.wal_pages, 0);
        drop(database);
        let database = JournalDatabase::open(&path).await?;
        assert_eq!(database.insert_batch(&[event(40)]).await?, 1);
        assert_eq!(database.read_cursor(None, None, 1).await?[0].0, 4);
        Ok(())
    }

    #[tokio::test]
    async fn pruning_and_reads_are_bounded() -> Result<(), TestError> {
        let directory = tempfile::tempdir()?;
        let database = JournalDatabase::open(&directory.path().join("journal.db")).await?;
        database
            .insert_batch(&(0..5002).map(event).collect::<Vec<_>>())
            .await?;
        assert_eq!(
            database.read_cursor(None, None, u16::MAX).await?.len(),
            1000
        );
        assert_eq!(database.prune(Prune::RowsBeyond(1)).await?, 5000);
        assert_eq!(database.prune(Prune::RowsBeyond(1)).await?, 1);
        assert_eq!(database.read_cursor(None, None, 100).await?.len(), 1);
        Ok(())
    }
}
