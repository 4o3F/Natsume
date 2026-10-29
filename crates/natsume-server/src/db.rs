use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use diesel::{
    Connection, RunQueryDsl,
    connection::{CacheSize, Instrumentation, InstrumentationEvent, SimpleConnection},
    r2d2::{ConnectionManager, CustomizeConnection, NopErrorHandler, Pool},
    sql_types::{Integer, Text},
    sqlite::SqliteConnection,
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use snafu::Snafu;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("diesel/business/migrations");
const DATABASE_CONNECTIONS: u32 = 10;
const SQLITE_PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=')
    .remove(b':')
    .remove(b'@')
    .remove(b'/');

pub(crate) struct DatabaseConfig {
    database_path: PathBuf,
    create_if_missing: bool,
}

impl DatabaseConfig {
    #[must_use]
    pub(crate) fn new(database_path: impl Into<PathBuf>, create_if_missing: bool) -> Self {
        Self {
            database_path: database_path.into(),
            create_if_missing,
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.database_path.is_absolute() && self.database_path.to_str().is_some()
    }
}

#[derive(Clone)]
pub(crate) struct Database {
    pool: Pool<ConnectionManager<SqliteConnection>>,
    work: Arc<Semaphore>,
}

/// An application-owned transaction boundary with the Diesel connection kept opaque.
pub(crate) struct Transaction<'a> {
    connection: &'a mut SqliteConnection,
}

impl Transaction<'_> {
    pub(crate) fn connection(&mut self) -> &mut SqliteConnection {
        self.connection
    }
}

#[derive(Debug, Snafu)]
pub(crate) enum PersistenceError {
    #[snafu(display("persisted data is invalid ({field})"))]
    InvalidPersistedData {
        field: &'static str,
        #[snafu(implicit)]
        location: snafu::Location,
    },
    #[snafu(display("unexpected affected row count (expected {expected}, actual {actual})"))]
    UnexpectedRowCount {
        expected: usize,
        actual: usize,
        #[snafu(implicit)]
        location: snafu::Location,
    },
    #[snafu(display("the persistence query failed"))]
    Query {
        source: diesel::result::Error,
        #[snafu(implicit)]
        location: snafu::Location,
    },
    #[snafu(display("the persistence connection pool failed"))]
    Pool {
        source: diesel::r2d2::PoolError,
        #[snafu(implicit)]
        location: snafu::Location,
    },
    #[snafu(display("the persistence executor failed"))]
    Executor {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(display("the persistence work budget is exhausted"))]
    WorkBudgetExhausted,
    #[snafu(display("persisted JSON is invalid ({field})"))]
    Json {
        field: &'static str,
        source: serde_json::Error,
    },
    #[snafu(display("persisted UUID is invalid ({field})"))]
    Uuid {
        field: &'static str,
        source: uuid::Error,
    },
    #[snafu(display("persisted integer is invalid ({field})"))]
    Integer {
        field: &'static str,
        source: std::num::TryFromIntError,
    },
    #[snafu(display("persisted text is invalid ({field})"))]
    Utf8 {
        field: &'static str,
        source: std::str::Utf8Error,
    },
    #[snafu(display("persisted address is invalid ({field})"))]
    Address {
        field: &'static str,
        source: std::net::AddrParseError,
    },
    #[snafu(display("persisted certificate is invalid ({field})"))]
    Certificate {
        field: &'static str,
        source: x509_parser::asn1_rs::Err<x509_parser::error::X509Error>,
    },
    #[snafu(display("persisted byte length is invalid ({field})"))]
    ByteLength {
        field: &'static str,
        source: std::array::TryFromSliceError,
    },
}

impl PersistenceError {
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            Self::Pool { .. } | Self::WorkBudgetExhausted => true,
            Self::Query {
                source:
                    diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::Unknown,
                        information,
                    ),
                ..
            } => {
                // Diesel exposes SQLite BUSY/LOCKED as Unknown with these SQLite messages.
                matches!(
                    information.message(),
                    "database is locked" | "database table is locked"
                )
            }
            _ => false,
        }
    }

    pub(crate) fn is_invalid_data(&self) -> bool {
        matches!(
            self,
            Self::InvalidPersistedData { .. }
                | Self::Json { .. }
                | Self::Uuid { .. }
                | Self::Integer { .. }
                | Self::Utf8 { .. }
                | Self::Address { .. }
                | Self::Certificate { .. }
                | Self::ByteLength { .. }
        )
    }

    #[track_caller]
    pub(crate) fn unexpected_row_count(expected: usize, actual: usize) -> Self {
        UnexpectedRowCountSnafu { expected, actual }.build()
    }

    #[track_caller]
    pub(crate) fn invalid(field: &'static str) -> Self {
        InvalidPersistedDataSnafu { field }.build()
    }
}

impl From<diesel::result::Error> for PersistenceError {
    #[track_caller]
    fn from(source: diesel::result::Error) -> Self {
        use snafu::IntoError as _;
        QuerySnafu.into_error(source)
    }
}

#[derive(Debug)]
pub(crate) enum TransactionError<E> {
    Operation(E),
    Persistence(PersistenceError),
}

impl<E> TransactionError<E> {
    pub(crate) fn into_error(self) -> E
    where
        E: From<PersistenceError>,
    {
        match self {
            Self::Operation(error) => error,
            Self::Persistence(error) => E::from(error),
        }
    }
}

impl<E> From<diesel::result::Error> for TransactionError<E> {
    fn from(source: diesel::result::Error) -> Self {
        Self::Persistence(source.into())
    }
}

impl Database {
    /// Connects to `SQLite` and applies embedded migrations.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`DatabaseError`] when configuration, connection,
    /// or migration fails.
    pub(crate) async fn connect_and_migrate(
        config: &DatabaseConfig,
    ) -> Result<Self, DatabaseError> {
        Self::connect_with(
            config,
            MIGRATIONS,
            DATABASE_CONNECTIONS,
            config.create_if_missing,
            Box::new(SqliteConnectionCustomizer),
        )
        .await
    }

    pub(crate) async fn connect_with(
        config: &DatabaseConfig,
        migrations: EmbeddedMigrations,
        pool_size: u32,
        create_if_missing: bool,
        customizer: Box<dyn CustomizeConnection<SqliteConnection, diesel::r2d2::Error>>,
    ) -> Result<Self, DatabaseError> {
        if !config.is_valid() || pool_size == 0 {
            return Err(DatabaseError::InvalidConfiguration);
        }
        let config = DatabaseConfig::new(&config.database_path, create_if_missing);
        let pool = build_pool(&config, pool_size, customizer)?;
        let migration_pool = pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = migration_pool
                .get()
                .map_err(|source| DatabaseError::ConnectionFailed { source })?;
            connection
                .run_pending_migrations(migrations)
                .map_err(|source| match source.downcast::<diesel::result::Error>() {
                    Ok(source) => DatabaseError::MigrationQuery { source: *source },
                    Err(source) => match source.downcast::<diesel_migrations::MigrationError>() {
                        Ok(source) => DatabaseError::MigrationSource { source: *source },
                        // Diesel's embedded migration query wrapper is private and has no source().
                        Err(error) => {
                            tracing::error!(error = %snafu::Report::from_error(error.as_ref()), "embedded migration failed");
                            DatabaseError::MigrationFailed
                        },
                    },
                })?;
            Ok::<(), DatabaseError>(())
        })
        .await
        .map_err(|source| DatabaseError::Executor { source: source.into() })??;
        Ok(Self {
            pool,
            work: Arc::new(Semaphore::new(pool_size as usize)),
        })
    }

    async fn reserve_work(&self) -> Result<(OwnedSemaphorePermit, Instant), PersistenceError> {
        let deadline = Instant::now() + self.pool.connection_timeout();
        let permit = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            Arc::clone(&self.work).acquire_owned(),
        )
        .await
        // Capacity timeout and semaphore closure both exhaust this work budget.
        .map_err(|_| PersistenceError::WorkBudgetExhausted)?
        // A closed capacity gate is represented by the same exhausted work budget.
        .map_err(|_| PersistenceError::WorkBudgetExhausted)?;
        Ok((permit, deadline))
    }

    pub(crate) async fn read<T, E, F>(&self, operation: F) -> Result<T, TransactionError<E>>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(&mut Transaction<'_>) -> Result<T, E> + Send + 'static,
    {
        let timing = transaction_timing_span("read");
        let waiting_since = wait_started(&timing);
        let reservation = self.reserve_work().await;
        record_wait(&timing, "db.permit_wait_us", waiting_since);
        let (permit, deadline) = reservation.map_err(TransactionError::Persistence)?;
        let pool = self.pool.clone();
        // A blocking thread inherits neither the scoped dispatcher nor the
        // current span. Capture both at this shared boundary so database work
        // remains associated with its entrypoint without an application ID.
        let dispatcher = tracing::dispatcher::get_default(Clone::clone);
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            // Cancellation of the caller must not free capacity while this work
            // is still queued or running in the blocking executor.
            let _permit = permit;
            tracing::dispatcher::with_default(&dispatcher, || {
                span.in_scope(|| {
                    let transaction_timing = timing;
                    let remaining = deadline.checked_duration_since(Instant::now()).ok_or(
                        TransactionError::Persistence(PersistenceError::WorkBudgetExhausted),
                    )?;
                    let waiting_since = wait_started(&transaction_timing);
                    let connection = pool.get_timeout(remaining);
                    record_wait(&transaction_timing, "db.pool_wait_us", waiting_since);
                    let mut connection = connection.map_err(|source| {
                        TransactionError::Persistence(PersistenceError::Pool {
                            source,
                            location: snafu::location!(),
                        })
                    })?;
                    connection.transaction::<T, TransactionError<E>, _>(|connection| {
                        let mut transaction = Transaction { connection };
                        operation(&mut transaction).map_err(TransactionError::Operation)
                    })
                })
            })
        })
        .await
        .map_err(|source| {
            TransactionError::Persistence(PersistenceError::Executor {
                source: source.into(),
            })
        })?
    }

    pub(crate) async fn write<T, E, F>(&self, operation: F) -> Result<T, TransactionError<E>>
    where
        T: Send + 'static,
        E: Send + 'static,
        F: FnOnce(&mut Transaction<'_>) -> Result<T, E> + Send + 'static,
    {
        let timing = transaction_timing_span("write");
        let waiting_since = wait_started(&timing);
        let reservation = self.reserve_work().await;
        record_wait(&timing, "db.permit_wait_us", waiting_since);
        let (permit, deadline) = reservation.map_err(TransactionError::Persistence)?;
        let pool = self.pool.clone();
        // A blocking thread inherits neither the scoped dispatcher nor the
        // current span. Capture both at this shared boundary so database work
        // remains associated with its entrypoint without an application ID.
        let dispatcher = tracing::dispatcher::get_default(Clone::clone);
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            tracing::dispatcher::with_default(&dispatcher, || {
                span.in_scope(|| {
                    let transaction_timing = timing;
                    let remaining = deadline.checked_duration_since(Instant::now()).ok_or(
                        TransactionError::Persistence(PersistenceError::WorkBudgetExhausted),
                    )?;
                    let waiting_since = wait_started(&transaction_timing);
                    let connection = pool.get_timeout(remaining);
                    record_wait(&transaction_timing, "db.pool_wait_us", waiting_since);
                    let mut connection = connection.map_err(|source| {
                        TransactionError::Persistence(PersistenceError::Pool {
                            source,
                            location: snafu::location!(),
                        })
                    })?;
                    let waiting_since = wait_started(&transaction_timing);
                    let mut acquired = false;
                    let result = connection.immediate_transaction::<T, TransactionError<E>, _>(
                        |connection| {
                            acquired = true;
                            record_wait(
                                &transaction_timing,
                                "db.write_lock_wait_us",
                                waiting_since,
                            );
                            let mut transaction = Transaction { connection };
                            operation(&mut transaction).map_err(TransactionError::Operation)
                        },
                    );
                    if !acquired {
                        record_wait(&transaction_timing, "db.write_lock_wait_us", waiting_since);
                    }
                    result
                })
            })
        })
        .await
        .map_err(|source| {
            TransactionError::Persistence(PersistenceError::Executor {
                source: source.into(),
            })
        })?
    }

    pub(crate) async fn journal_maintenance(
        &self,
        truncate: bool,
    ) -> Result<(i32, i32, i32, i64), PersistenceError> {
        let (permit, deadline) = self.reserve_work().await?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(PersistenceError::WorkBudgetExhausted)?;
            let mut connection =
                pool.get_timeout(remaining)
                    .map_err(|source| PersistenceError::Pool {
                        source,
                        location: snafu::location!(),
                    })?;
            // SQLite checkpoint and vacuum must run in autocommit; Diesel has no typed pragma API.
            let checkpoint = if truncate {
                "PRAGMA wal_checkpoint(TRUNCATE)"
            } else {
                "PRAGMA wal_checkpoint(PASSIVE)"
            };
            let (busy, log, checkpointed) =
                diesel::dsl::sql::<(Integer, Integer, Integer)>(checkpoint)
                    .get_result::<(i32, i32, i32)>(&mut connection)?;
            connection.batch_execute("PRAGMA incremental_vacuum(5000)")?;
            let pages = diesel::dsl::sql::<diesel::sql_types::BigInt>("PRAGMA page_count")
                .get_result::<i64>(&mut connection)?;
            Ok((busy, log, checkpointed, pages))
        })
        .await
        .map_err(|source| PersistenceError::Executor {
            source: source.into(),
        })?
    }
}

fn transaction_timing_span(operation: &'static str) -> tracing::Span {
    tracing::debug_span!(
        target: "natsume_server::db",
        "db.transaction",
        "db.operation" = operation,
        "db.permit_wait_us" = tracing::field::Empty,
        "db.pool_wait_us" = tracing::field::Empty,
        "db.write_lock_wait_us" = tracing::field::Empty,
    )
}

fn wait_started(span: &tracing::Span) -> Option<Instant> {
    (!span.is_disabled()).then(Instant::now)
}

fn record_wait(span: &tracing::Span, field: &'static str, started: Option<Instant>) {
    if let Some(started) = started {
        span.record(
            field,
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
    }
}

fn build_pool(
    config: &DatabaseConfig,
    pool_size: u32,
    customizer: Box<dyn CustomizeConnection<SqliteConnection, diesel::r2d2::Error>>,
) -> Result<Pool<ConnectionManager<SqliteConnection>>, DatabaseError> {
    let manager = ConnectionManager::new(diesel_database_url(config)?);
    Pool::builder()
        .max_size(pool_size)
        .min_idle(Some(0))
        .connection_timeout(Duration::from_secs(30))
        .idle_timeout(Some(Duration::from_mins(10)))
        .max_lifetime(Some(Duration::from_mins(30)))
        .test_on_check_out(true)
        .error_handler(Box::new(NopErrorHandler))
        .connection_customizer(customizer)
        .build(manager)
        .map_err(|source| DatabaseError::ConnectionFailed { source })
}

fn diesel_database_url(config: &DatabaseConfig) -> Result<String, DatabaseError> {
    if !config.is_valid() {
        return Err(DatabaseError::InvalidConfiguration);
    }
    let path = config
        .database_path
        .to_str()
        .ok_or(DatabaseError::InvalidConfiguration)?;
    let encoded = utf8_percent_encode(path, SQLITE_PATH_ENCODE_SET).collect::<String>();
    let mode = if config.create_if_missing {
        "rwc"
    } else {
        "rw"
    };
    Ok(format!("sqlite://{encoded}?mode={mode}"))
}

#[derive(Debug)]
struct SqliteConnectionCustomizer;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for SqliteConnectionCustomizer {
    fn on_acquire(&self, connection: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        connection
            .batch_execute(
                "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
            )
            .map_err(diesel::r2d2::Error::QueryError)?;
        connection.set_prepared_statement_cache_size(CacheSize::Unbounded);
        let (foreign_keys, journal_mode, busy_timeout) =
            sqlite_pragma_values(connection).map_err(diesel::r2d2::Error::QueryError)?;
        if foreign_keys != 1 || journal_mode != "wal" || busy_timeout != 5_000 {
            return Err(diesel::r2d2::Error::QueryError(
                diesel::result::Error::NotFound,
            ));
        }
        connection.set_instrumentation(SqliteQueryInstrumentation::default());
        Ok(())
    }
}

#[derive(Default)]
struct SqliteQueryInstrumentation {
    active_query: Option<tracing::Span>,
}

impl Instrumentation for SqliteQueryInstrumentation {
    fn on_connection_event(&mut self, event: InstrumentationEvent<'_>) {
        match event {
            InstrumentationEvent::StartQuery { .. } => {
                self.active_query = Some(tracing::debug_span!(
                    target: "natsume_server::db",
                    "db.query",
                    "otel.kind" = "client",
                    "otel.status_code" = tracing::field::Empty,
                    "db.system.name" = "sqlite",
                    "error.type" = tracing::field::Empty,
                ));
            }
            InstrumentationEvent::FinishQuery { error, .. } => {
                if let Some(span) = self.active_query.take()
                    && error.is_some()
                {
                    span.record("otel.status_code", "ERROR");
                    span.record("error.type", "diesel");
                }
            }
            _ => {}
        }
    }
}

fn sqlite_pragma_values(
    connection: &mut SqliteConnection,
) -> diesel::QueryResult<(i32, String, i32)> {
    let foreign_keys = diesel::dsl::sql::<Integer>("PRAGMA foreign_keys").get_result(connection)?;
    let journal_mode = diesel::dsl::sql::<Text>("PRAGMA journal_mode").get_result(connection)?;
    let busy_timeout = diesel::dsl::sql::<Integer>("PRAGMA busy_timeout").get_result(connection)?;
    Ok((foreign_keys, journal_mode, busy_timeout))
}

#[derive(Debug, Snafu)]
#[snafu(module)]
pub(crate) enum DatabaseError {
    #[snafu(display("the database configuration is invalid"))]
    InvalidConfiguration,
    #[snafu(display("the database file could not be opened"))]
    File { source: std::io::Error },
    #[snafu(display("the database connection failed"))]
    ConnectionFailed { source: diesel::r2d2::PoolError },
    #[snafu(display("the database migration failed"))]
    MigrationFailed,
    #[snafu(display("the database migration query failed"))]
    MigrationQuery { source: diesel::result::Error },
    #[snafu(display("the database migration source failed"))]
    MigrationSource {
        source: diesel_migrations::MigrationError,
    },
    #[snafu(display("the database executor failed"))]
    Executor {
        #[snafu(source(from(tokio::task::JoinError, crate::task::TaskFailure::from)))]
        source: crate::task::TaskFailure,
    },
    #[snafu(display("the database observer connection failed"))]
    #[cfg(test)]
    Observer { source: diesel::ConnectionError },
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc, Condvar, Mutex, PoisonError,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use diesel::{
        Connection, RunQueryDsl, connection::SimpleConnection, sql_types::BigInt,
        sqlite::SqliteConnection,
    };
    use snafu::Snafu;
    use tracing::{
        Instrument as _, Subscriber,
        field::{Field, Visit},
        instrument::WithSubscriber as _,
        span::{Attributes, Id, Record},
    };
    use tracing_subscriber::{
        Layer, Registry,
        layer::{Context, SubscriberExt as _},
    };
    use uuid::Uuid;

    use crate::logging::tests::SubscriberTestGuard;

    use super::{
        DATABASE_CONNECTIONS, Database, DatabaseConfig, DatabaseError, PersistenceError,
        TransactionError, diesel_database_url, sqlite_pragma_values,
    };

    impl Database {
        pub(crate) fn pool_size_for_test(&self) -> u32 {
            self.pool.max_size()
        }

        pub(crate) fn test_exhaust_pool(
            &self,
        ) -> Vec<diesel::r2d2::PooledConnection<diesel::r2d2::ConnectionManager<SqliteConnection>>>
        {
            (0..self.pool.max_size())
                .map(|_| {
                    self.pool
                        .get()
                        .unwrap_or_else(|error| panic!("test pool checkout failed: {error}"))
                })
                .collect()
        }

        pub(crate) async fn test_read<T, F>(&self, operation: F) -> Result<T, PersistenceError>
        where
            T: Send + 'static,
            F: FnOnce(&mut SqliteConnection) -> T + Send + 'static,
        {
            self.read(move |transaction| {
                Ok::<T, PersistenceError>(operation(transaction.connection()))
            })
            .await
            .map_err(TransactionError::into_error)
        }
    }

    #[test]
    fn embedded_business_migration_versions_are_frozen() {
        let migrations = diesel::migration::MigrationSource::<diesel::sqlite::Sqlite>::migrations(
            &super::MIGRATIONS,
        )
        .unwrap_or_else(|error| panic!("embedded business migrations unavailable: {error}"));
        let versions = migrations
            .iter()
            .map(|migration| migration.name().version().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            versions,
            [
                "00000000000001",
                "00000000000002",
                "00000000000003",
                "00000000000004",
                "00000000000005",
                "00000000000006",
                "00000000000007",
                "00000000000008",
            ]
        );
    }

    #[tokio::test]
    async fn reopening_business_database_preserves_data_without_pending_migrations()
    -> Result<(), crate::test_error::TestError> {
        use crate::diesel_schema::seats;
        use diesel::{ExpressionMethods, QueryDsl};
        use diesel_migrations::MigrationHarness as _;

        let fixture = DatabaseFixture::new();
        let database =
            Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true)).await?;
        let seat_id = Uuid::now_v7().to_string();
        let inserted_id = seat_id.clone();
        database
            .write(move |transaction| {
                diesel::insert_into(seats::table)
                    .values((
                        seats::seat_id.eq(inserted_id),
                        seats::seat_code.eq("layout-preserved"),
                    ))
                    .execute(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await?;
        drop(database);

        let database =
            Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, false)).await?;
        let pending = database
            .test_read(|connection| {
                connection
                    .pending_migrations(super::MIGRATIONS)
                    .unwrap_or_else(|error| {
                        panic!("pending business migrations unavailable: {error}")
                    })
                    .len()
            })
            .await?;
        assert_eq!(pending, 0);
        let seat_code = database
            .read(move |transaction| {
                seats::table
                    .filter(seats::seat_id.eq(seat_id))
                    .select(seats::seat_code)
                    .first::<String>(transaction.connection())
                    .map_err(PersistenceError::from)
            })
            .await?;
        assert_eq!(seat_code, "layout-preserved");
        Ok(())
    }

    struct HeldWork(Arc<(Mutex<bool>, Condvar)>);

    impl HeldWork {
        fn new() -> Self {
            Self(Arc::new((Mutex::new(false), Condvar::new())))
        }

        fn wait(gate: &Arc<(Mutex<bool>, Condvar)>) {
            let released = gate.0.lock().unwrap_or_else(PoisonError::into_inner);
            drop(
                gate.1
                    .wait_while(released, |released| !*released)
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    impl Drop for HeldWork {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
            self.0.1.notify_all();
        }
    }

    async fn fixture_database(fixture: &DatabaseFixture) -> Database {
        Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .unwrap_or_else(|error| panic!("fixture database: {error}"))
    }

    #[test]
    fn clones_bound_blocking_threads_before_pool_checkout() {
        let threads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&threads);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(64)
            .on_thread_start(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            })
            .build()
            .unwrap_or_else(|error| panic!("test runtime: {error}"));
        runtime.block_on(async {
            let fixture = DatabaseFixture::new();
            let database = fixture_database(&fixture).await;
            let held = HeldWork::new();
            let (entered, mut started) = tokio::sync::mpsc::unbounded_channel();
            let mut tasks = Vec::new();
            for _ in 0..128 {
                let database = database.clone();
                let gate = Arc::clone(&held.0);
                let entered = entered.clone();
                tasks.push(tokio::spawn(async move {
                    database
                        .read(move |_| {
                            let _ = entered.send(());
                            HeldWork::wait(&gate);
                            Ok::<(), PersistenceError>(())
                        })
                        .await
                }));
            }
            for _ in 0..DATABASE_CONNECTIONS {
                assert!(
                    tokio::time::timeout(Duration::from_secs(5), started.recv())
                        .await
                        .unwrap_or_else(|error| panic!("work did not start: {error}"))
                        .is_some()
                );
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(database.work.available_permits(), 0);
            assert!(threads.load(Ordering::SeqCst) <= DATABASE_CONNECTIONS as usize);
            drop(held);
            for task in tasks {
                assert!(matches!(
                    task.await
                        .unwrap_or_else(|error| panic!("database task: {error}")),
                    Ok(())
                ));
            }
            assert_eq!(
                database.work.available_permits(),
                DATABASE_CONNECTIONS as usize
            );
        });
    }

    #[tokio::test]
    async fn cancelled_callers_do_not_release_running_work_or_execute_queued_work() {
        let fixture = DatabaseFixture::new();
        let mut database = fixture_database(&fixture).await;
        database.work = Arc::new(tokio::sync::Semaphore::new(1));
        database
            .write(|transaction| {
                transaction
                    .connection()
                    .batch_execute("CREATE TABLE work_probe (value INTEGER NOT NULL)")
                    .map_err(PersistenceError::from)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("create probe table: {error:?}"));
        let held = HeldWork::new();
        let gate = Arc::clone(&held.0);
        let (entered, started) = tokio::sync::oneshot::channel();
        let writer = database.clone();
        let task = tokio::spawn(async move {
            writer
                .write(move |transaction| {
                    transaction
                        .connection()
                        .batch_execute("INSERT INTO work_probe VALUES (1)")
                        .map_err(PersistenceError::from)?;
                    let _ = entered.send(());
                    HeldWork::wait(&gate);
                    Ok::<(), PersistenceError>(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap_or_else(|error| panic!("work timeout: {error}"))
            .unwrap_or_else(|error| panic!("work signal: {error}"));
        task.abort();
        assert!(task.await.is_err());
        assert_eq!(database.work.available_permits(), 0);
        let executed = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&executed);
        let reader = database.clone();
        let queued = tokio::spawn(async move {
            reader
                .read(move |_| {
                    observed.store(true, Ordering::SeqCst);
                    Ok::<(), PersistenceError>(())
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!queued.is_finished());
        queued.abort();
        assert!(queued.await.is_err());
        drop(held);
        let count = database
            .read(|transaction| {
                diesel::dsl::sql::<BigInt>("SELECT COUNT(*) FROM work_probe")
                    .get_result::<i64>(transaction.connection())
            })
            .await
            .unwrap_or_else(|error| panic!("read committed result: {error:?}"));
        assert_eq!(count, 1);
        assert!(!executed.load(Ordering::SeqCst));
        assert_eq!(database.work.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn admission_timeout_does_not_start_sql() {
        let fixture = DatabaseFixture::new();
        let database = fixture_database(&fixture).await;
        let permit = Arc::clone(&database.work)
            .acquire_many_owned(DATABASE_CONNECTIONS)
            .await
            .unwrap_or_else(|error| panic!("hold capacity: {error}"));
        let executed = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&executed);
        let waiting = database.clone();
        let task = tokio::spawn(async move {
            waiting
                .read(move |_| {
                    observed.store(true, Ordering::SeqCst);
                    Ok::<(), PersistenceError>(())
                })
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(matches!(
            task.await
                .unwrap_or_else(|error| panic!("waiting task: {error}")),
            Err(TransactionError::Persistence(
                PersistenceError::WorkBudgetExhausted
            ))
        ));
        assert!(!executed.load(Ordering::SeqCst));
        drop(permit);
    }

    #[test]
    fn blocking_queue_time_counts_toward_connection_budget() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap_or_else(|error| panic!("runtime: {error}"));
        runtime.block_on(async {
            let fixture = DatabaseFixture::new();
            let mut database = fixture_database(&fixture).await;
            let manager = diesel::r2d2::ConnectionManager::new(
                diesel_database_url(&DatabaseConfig::new(&fixture.path, false))
                    .unwrap_or_else(|error| panic!("database URL: {error}")),
            );
            database.pool = diesel::r2d2::Pool::builder()
                .max_size(1)
                .min_idle(Some(0))
                .connection_timeout(Duration::from_millis(50))
                .build(manager)
                .unwrap_or_else(|error| panic!("short timeout pool: {error}"));
            let held = HeldWork::new();
            let gate = Arc::clone(&held.0);
            let (entered, started) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = entered.send(());
                HeldWork::wait(&gate);
            });
            started
                .await
                .unwrap_or_else(|error| panic!("blocking task: {error}"));
            let queued = database.clone();
            let task =
                tokio::spawn(async move { queued.read(|_| Ok::<(), PersistenceError>(())).await });
            tokio::task::yield_now().await;
            assert_eq!(
                database.work.available_permits(),
                DATABASE_CONNECTIONS as usize - 1
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(held);
            blocker
                .await
                .unwrap_or_else(|error| panic!("blocking task: {error}"));
            assert!(matches!(
                task.await
                    .unwrap_or_else(|error| panic!("queued work: {error}")),
                Err(TransactionError::Persistence(
                    PersistenceError::WorkBudgetExhausted
                ))
            ));
            assert_eq!(
                database.work.available_permits(),
                DATABASE_CONNECTIONS as usize
            );
        });
    }

    #[tokio::test]
    async fn errors_roll_back_and_panics_release_database_capacity() {
        let fixture = DatabaseFixture::new();
        let database = fixture_database(&fixture).await;
        let failed = database
            .write(|transaction| {
                transaction
                    .connection()
                    .batch_execute("CREATE TABLE rolled_back (value INTEGER)")
                    .map_err(PersistenceError::from)?;
                Err::<(), PersistenceError>(PersistenceError::invalid("persisted value"))
            })
            .await;
        assert!(matches!(
            failed,
            Err(TransactionError::Operation(
                PersistenceError::InvalidPersistedData { .. }
            ))
        ));
        let tables = database
            .read(|transaction| {
                diesel::dsl::sql::<BigInt>(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = 'rolled_back'",
                )
                .get_result::<i64>(transaction.connection())
            })
            .await
            .unwrap_or_else(|error| panic!("rollback probe: {error:?}"));
        assert_eq!(tables, 0);
        let failed = database
            .read::<(), PersistenceError, _>(|_| panic!("intentional database worker panic"))
            .await;
        assert!(matches!(
            failed,
            Err(TransactionError::Persistence(
                PersistenceError::Executor { .. }
            ))
        ));
        assert_eq!(
            database.work.available_permits(),
            DATABASE_CONNECTIONS as usize
        );
        assert!(
            matches!(database.read(|_| Ok::<_, PersistenceError>(42)).await, Ok(actual) if actual == 42)
        );
    }

    /// Opens an independent connection outside the pool so tests can observe
    /// committed state without joining the pool's own contention.
    pub(crate) fn test_observer(path: &Path) -> Result<SqliteConnection, DatabaseError> {
        let path = path.to_str().ok_or(DatabaseError::InvalidConfiguration)?;
        SqliteConnection::establish(path).map_err(|source| DatabaseError::Observer { source })
    }

    /// Holds the `SQLite` write lock for as long as the returned connection
    /// lives, so tests can prove which paths need it.
    pub(crate) fn test_lock_database(path: &Path) -> Result<SqliteConnection, DatabaseError> {
        let mut connection = test_observer(path)?;
        connection
            .batch_execute("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .map_err(|source| DatabaseError::MigrationQuery { source })?;
        Ok(connection)
    }

    /// Reads the observer's `PRAGMA data_version`, which advances only when
    /// another connection commits a write.
    pub(crate) fn test_data_version(
        connection: &mut SqliteConnection,
    ) -> Result<i64, DatabaseError> {
        diesel::dsl::sql::<BigInt>("PRAGMA data_version")
            .get_result(connection)
            .map_err(|source| DatabaseError::MigrationQuery { source })
    }

    #[tokio::test]
    async fn transient_persistence_errors_are_only_capacity_and_sqlite_lock_failures() {
        use diesel::result::{DatabaseErrorKind, Error};
        use snafu::IntoError as _;

        for (kind, message, expected) in [
            (DatabaseErrorKind::Unknown, "database is locked", true),
            (DatabaseErrorKind::Unknown, "database table is locked", true),
            (DatabaseErrorKind::Unknown, "disk I/O error", false),
            (DatabaseErrorKind::Unknown, "SQLITE_BUSY", false),
            (
                DatabaseErrorKind::UniqueViolation,
                "database is locked",
                false,
            ),
        ] {
            let error =
                PersistenceError::from(Error::DatabaseError(kind, Box::new(message.to_owned())));
            assert_eq!(error.is_transient(), expected, "{message}");
        }
        assert!(!PersistenceError::from(Error::NotFound).is_transient());
        assert!(!PersistenceError::invalid("test field").is_transient());
        assert!(PersistenceError::WorkBudgetExhausted.is_transient());

        let fixture = DatabaseFixture::new();
        let database = fixture_database(&fixture).await;
        let _connections = database.test_exhaust_pool();
        let Err(source) = database.pool.get_timeout(Duration::ZERO) else {
            panic!("the exhausted test pool must reject acquisition");
        };
        assert!(super::PoolSnafu.into_error(source).is_transient());
    }

    #[tokio::test]
    async fn locked_write_preserves_the_sqlite_query_source_and_report() {
        let fixture = DatabaseFixture::new();
        let database = fixture_database(&fixture).await;
        let _lock = test_lock_database(&fixture.path)
            .unwrap_or_else(|error| panic!("{}", snafu::Report::from_error(error)));
        let result = database.write(|_| Ok::<(), PersistenceError>(())).await;
        let Err(TransactionError::Persistence(error)) = result else {
            panic!("the locked write must fail before its operation");
        };
        assert!(error.is_transient());
        let PersistenceError::Query {
            source: diesel::result::Error::DatabaseError(_, ref information),
            ..
        } = error
        else {
            panic!("the SQLite query source must be preserved: {error:?}");
        };
        let message = information.message();
        assert!(message.contains("locked") || message.contains("busy"));
        assert!(
            snafu::Report::from_error(&error)
                .to_string()
                .contains(message)
        );
    }

    #[tokio::test]
    async fn missing_database_is_not_created_when_creation_is_disabled() {
        let fixture = DatabaseFixture::new();
        let error =
            match Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, false)).await {
                Err(error) => error,
                Ok(database) => {
                    drop(database);
                    panic!("a missing database unexpectedly opened")
                }
            };

        assert!(matches!(error, DatabaseError::ConnectionFailed { .. }));
        for path in [&fixture.path, &fixture.wal_path(), &fixture.shm_path()] {
            assert!(!path.exists(), "serve-mode database artifacts were created");
        }

        let display = error.to_string();
        let debug = format!("{error:?}");
        let canary = fixture.path.to_string_lossy();
        assert!(!display.contains(canary.as_ref()));
        assert!(!debug.contains(canary.as_ref()));
    }

    #[test]
    fn transaction_error_preserves_operation_and_persistence_failures() {
        let operation = TransactionError::Operation(PersistenceError::invalid("persisted value"));
        assert!(matches!(
            operation,
            TransactionError::Operation(PersistenceError::InvalidPersistedData { .. })
        ));
        assert!(matches!(
            operation.into_error(),
            PersistenceError::InvalidPersistedData { .. }
        ));

        let persistence =
            TransactionError::<PersistenceError>::from(diesel::result::Error::RollbackTransaction);
        assert!(matches!(
            persistence,
            TransactionError::Persistence(PersistenceError::Query { .. })
        ));
        assert!(matches!(
            persistence.into_error(),
            PersistenceError::Query { .. }
        ));
    }

    #[tokio::test]
    async fn diesel_pool_connections_have_exact_pragmas() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let pragmas = database
            .test_read(sqlite_pragma_values)
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if pragmas != (1, "wal".to_owned(), 5_000) {
            return Err(TestFailure::PragmasWereNotExact);
        }
        drop(database);
        Ok(())
    }

    #[tokio::test]
    async fn diesel_pool_has_exact_limits_and_does_not_preopen_to_capacity()
    -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let pool = &database.pool;
        let state = pool.state();
        if pool.max_size() != 10
            || pool.min_idle() != Some(0)
            || pool.connection_timeout() != Duration::from_secs(30)
            || pool.idle_timeout() != Some(Duration::from_mins(10))
            || pool.max_lifetime() != Some(Duration::from_mins(30))
            || !pool.test_on_check_out()
            || state.connections != 1
            || state.idle_connections != 1
        {
            return Err(TestFailure::PoolConfigurationWasNotExact);
        }
        drop(database);
        Ok(())
    }

    #[tokio::test]
    async fn read_and_write_propagate_the_current_span_to_blocking_work() -> Result<(), TestFailure>
    {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let fixture = DatabaseFixture::new();
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;

        let (read_span_matches, write_span_matches) = async {
            let span = tracing::info_span!("database_span_propagation_test");
            let expected_id = span.id().ok_or(TestFailure::ContextSpanDisabled)?;
            let read_expected_id = expected_id.clone();
            async {
                let read_span_matches = database
                    .read(move |_transaction| {
                        Ok::<bool, DatabaseError>(
                            tracing::Span::current().id().as_ref() == Some(&read_expected_id),
                        )
                    })
                    .await
                    .map_err(|source| {
                        TestFailure::from(crate::test_error::TestError::from(source))
                    })?;
                let write_span_matches = database
                    .write(move |_transaction| {
                        Ok::<bool, DatabaseError>(
                            tracing::Span::current().id().as_ref() == Some(&expected_id),
                        )
                    })
                    .await
                    .map_err(|source| {
                        TestFailure::from(crate::test_error::TestError::from(source))
                    })?;
                Ok::<(bool, bool), TestFailure>((read_span_matches, write_span_matches))
            }
            .instrument(span)
            .await
        }
        .with_subscriber(Registry::default())
        .await?;

        if !read_span_matches || !write_span_matches {
            return Err(TestFailure::DatabaseSpanWasNotPropagated);
        }
        Ok(())
    }

    #[tokio::test]
    async fn diesel_instrumentation_emits_query_spans_without_sql_or_bind_values()
    -> Result<(), TestFailure> {
        let _subscriber_guard = SubscriberTestGuard::acquire();
        let fixture = DatabaseFixture::new();
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let captured = CapturedSqlSpans::default();
        let subscriber = Registry::default().with(captured.clone());

        async {
            database
                .test_read(|connection| {
                    diesel::select(diesel::dsl::sql::<diesel::sql_types::Text>(
                        "'sql-secret-canary'",
                    ))
                    .get_result::<String>(connection)
                })
                .await
                .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?
                .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
            Ok::<(), TestFailure>(())
        }
        .with_subscriber(subscriber)
        .await?;

        let evidence = captured.snapshot();
        if evidence.query_span_count == 0 || evidence.secret_was_recorded {
            return Err(TestFailure::DatabaseQuerySpanChanged);
        }
        Ok(())
    }

    #[derive(Clone, Default)]
    struct CapturedWaitSpans {
        rows: Arc<Mutex<Vec<WaitEvidence>>>,
    }

    #[derive(Clone, Default)]
    struct WaitEvidence {
        id: u64,
        operation: String,
        permit_wait_us: Option<u64>,
        pool_wait_us: Option<u64>,
        write_lock_wait_us: Option<u64>,
        secret_was_recorded: bool,
    }

    impl CapturedWaitSpans {
        fn snapshot(&self) -> Vec<WaitEvidence> {
            self.rows
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl<S: Subscriber> Layer<S> for CapturedWaitSpans {
        fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, _context: Context<'_, S>) {
            if attributes.metadata().name() == "db.transaction" {
                let mut row = WaitEvidence {
                    id: id.into_u64(),
                    ..WaitEvidence::default()
                };
                attributes.record(&mut row);
                self.rows
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(row);
            }
        }

        fn on_record(&self, id: &Id, values: &Record<'_>, _context: Context<'_, S>) {
            if let Some(row) = self
                .rows
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter_mut()
                .find(|row| row.id == id.into_u64())
            {
                values.record(row);
            }
        }
    }

    impl Visit for WaitEvidence {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "db.operation" {
                self.operation = value.to_owned();
            }
            self.secret_was_recorded |= value.contains("db-timing-secret-canary");
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            match field.name() {
                "db.permit_wait_us" => self.permit_wait_us = Some(value),
                "db.pool_wait_us" => self.pool_wait_us = Some(value),
                "db.write_lock_wait_us" => self.write_lock_wait_us = Some(value),
                _ => {}
            }
        }

        fn record_debug(&mut self, _field: &Field, value: &dyn std::fmt::Debug) {
            self.secret_was_recorded |= format!("{value:?}").contains("db-timing-secret-canary");
        }
    }

    async fn wait_for_timing_stage(
        captured: &CapturedWaitSpans,
        index: usize,
        ready: impl Fn(&WaitEvidence) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if captured.snapshot().get(index).is_some_and(&ready) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timing stage did not start"));
    }

    #[tokio::test]
    async fn transaction_timings_distinguish_permit_pool_and_write_lock_waits()
    -> Result<(), crate::test_error::TestError> {
        let _guard = SubscriberTestGuard::acquire();
        let fixture = DatabaseFixture::new();
        let database = fixture_database(&fixture).await;
        let captured = CapturedWaitSpans::default();
        async {
            let permits = Arc::clone(&database.work)
                .acquire_many_owned(DATABASE_CONNECTIONS)
                .await?;
            let connection = database.clone();
            let queued = tokio::spawn(
                async move {
                    connection
                        .read(|_| Ok::<_, PersistenceError>("db-timing-secret-canary"))
                        .await
                }
                .with_current_subscriber(),
            );
            wait_for_timing_stage(&captured, 0, |_| true).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(permits);
            assert_eq!(queued.await??, "db-timing-secret-canary");
            assert!(
                captured.snapshot()[0]
                    .permit_wait_us
                    .is_some_and(|wait| wait >= 10_000)
            );

            let connections = database.test_exhaust_pool();
            let connection = database.clone();
            let queued = tokio::spawn(
                async move { connection.read(|_| Ok::<_, PersistenceError>(())).await }
                    .with_current_subscriber(),
            );
            wait_for_timing_stage(&captured, 1, |row| row.permit_wait_us.is_some()).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(connections);
            queued.await??;
            assert!(
                captured.snapshot()[1]
                    .pool_wait_us
                    .is_some_and(|wait| wait >= 10_000)
            );

            let lock = test_lock_database(&fixture.path)?;
            let connection = database.clone();
            let queued = tokio::spawn(
                async move { connection.write(|_| Ok::<_, PersistenceError>(())).await }
                    .with_current_subscriber(),
            );
            wait_for_timing_stage(&captured, 2, |row| row.pool_wait_us.is_some()).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(lock);
            queued.await??;
            assert!(
                captured.snapshot()[2]
                    .write_lock_wait_us
                    .is_some_and(|wait| wait >= 10_000)
            );

            for mut connection in database.test_exhaust_pool() {
                connection.batch_execute("PRAGMA busy_timeout = 0;")?;
            }
            let _lock = test_lock_database(&fixture.path)?;
            let failure = database.write(|_| Ok::<_, PersistenceError>(())).await;
            assert!(matches!(
                failure,
                Err(TransactionError::Persistence(
                    PersistenceError::Query { .. }
                ))
            ));
            Ok::<_, crate::test_error::TestError>(())
        }
        .with_subscriber(Registry::default().with(captured.clone()))
        .await?;
        let rows = captured.snapshot();
        assert_eq!(rows.len(), 4);
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row.operation, if index < 2 { "read" } else { "write" });
            assert!(row.permit_wait_us.is_some());
            assert!(row.pool_wait_us.is_some());
            assert_eq!(row.write_lock_wait_us.is_some(), index >= 2);
            assert!(!row.secret_was_recorded);
        }
        Ok(())
    }

    #[derive(Clone, Default)]
    struct CapturedSqlSpans {
        evidence: Arc<Mutex<SqlSpanEvidence>>,
    }

    impl CapturedSqlSpans {
        fn snapshot(&self) -> SqlSpanEvidence {
            self.evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl<S> Layer<S> for CapturedSqlSpans
    where
        S: Subscriber,
    {
        fn on_new_span(&self, attributes: &Attributes<'_>, _id: &Id, _context: Context<'_, S>) {
            let name = attributes.metadata().name();
            if name != "db.query" && name != "db.transaction" {
                return;
            }
            let mut evidence = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
            if name == "db.query" {
                evidence.query_span_count += 1;
            }
            attributes.record(&mut SqlSpanVisitor {
                evidence: &mut evidence,
            });
        }
    }

    #[derive(Clone, Default)]
    struct SqlSpanEvidence {
        query_span_count: usize,
        secret_was_recorded: bool,
    }

    struct SqlSpanVisitor<'evidence> {
        evidence: &'evidence mut SqlSpanEvidence,
    }

    impl Visit for SqlSpanVisitor<'_> {
        fn record_debug(&mut self, _field: &Field, value: &dyn std::fmt::Debug) {
            if format!("{value:?}").contains("sql-secret-canary") {
                self.evidence.secret_was_recorded = true;
            }
        }
    }

    #[test]
    fn diesel_read_write_mode_does_not_create_a_missing_database() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let config = DatabaseConfig::new(&fixture.path, false);
        let database_url = diesel_database_url(&config)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        if let Ok(connection) = diesel::sqlite::SqliteConnection::establish(&database_url) {
            drop(connection);
            return Err(TestFailure::MissingDatabaseWasOpened);
        }
        for path in [&fixture.path, &fixture.wal_path(), &fixture.shm_path()] {
            if path.exists() {
                return Err(TestFailure::MissingDatabaseArtifactWasCreated);
            }
        }
        let error = DatabaseError::Observer {
            source: diesel::ConnectionError::BadConnection("connection failed".to_owned()),
        };
        let canary = fixture.path.to_string_lossy();
        let display = error.to_string();
        let debug = format!("{error:?}");
        if display.contains(canary.as_ref()) || debug.contains(canary.as_ref()) {
            return Err(TestFailure::DatabaseErrorWasNotRedacted);
        }
        Ok(())
    }

    #[test]
    fn diesel_database_url_uses_the_frozen_path_encoding_set() -> Result<(), TestFailure> {
        let path = PathBuf::from("/tmp/Natsume URL#%?汉字-._~!$&'()*+,;=:@/db.sqlite3");
        let read_write = diesel_database_url(&DatabaseConfig::new(&path, false))
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let create = diesel_database_url(&DatabaseConfig::new(&path, true))
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let encoded =
            "sqlite:///tmp/Natsume%20URL%23%25%3F%E6%B1%89%E5%AD%97-._~!$&'()*+,;=:@/db.sqlite3";
        if read_write != format!("{encoded}?mode=rw") || create != format!("{encoded}?mode=rwc") {
            return Err(TestFailure::DatabaseUrlEncodingChanged);
        }
        Ok(())
    }

    #[test]
    fn begin_immediate_reserves_write_before_the_first_statement() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let path = fixture
            .path
            .to_str()
            .ok_or(TestFailure::DatabasePathWasNotUtf8)?;
        let mut owner = diesel::sqlite::SqliteConnection::establish(path)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        let mut competitor = diesel::sqlite::SqliteConnection::establish(path)
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;
        competitor
            .batch_execute("PRAGMA busy_timeout = 0;")
            .map_err(|source| TestFailure::from(crate::test_error::TestError::from(source)))?;

        owner.immediate_transaction(|_connection| {
            if competitor.batch_execute("BEGIN IMMEDIATE;").is_ok() {
                competitor.batch_execute("ROLLBACK;").map_err(|source| {
                    TestFailure::from(crate::test_error::TestError::from(source))
                })?;
                return Err(TestFailure::ImmediateTransactionDidNotReserveWrite);
            }
            Ok(())
        })
    }

    impl From<diesel::result::Error> for TestFailure {
        fn from(source: diesel::result::Error) -> Self {
            Self::from(crate::test_error::TestError::from(source))
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(context(false), display("test infrastructure operation failed"))]
        Source {
            #[snafu(source(from(crate::test_error::TestError, Box::new)))]
            source: Box<crate::test_error::TestError>,
        },
        #[snafu(display("the test database could not be created"))]
        DatabaseCreationFailed,
        #[snafu(display("the Diesel interaction failed"))]
        DieselInteractionFailed,
        #[snafu(display("the database context test span was disabled"))]
        ContextSpanDisabled,
        #[snafu(display("the current span did not reach blocking database work"))]
        DatabaseSpanWasNotPropagated,
        #[snafu(display("the redacted Diesel query span contract changed"))]
        DatabaseQuerySpanChanged,
        #[snafu(display("the Diesel connection PRAGMAs were not exact"))]
        PragmasWereNotExact,
        #[snafu(display("the Diesel connection PRAGMAs were not readable"))]
        PragmasWereNotReadable,
        #[snafu(display("the Diesel pool configuration was not exact"))]
        PoolConfigurationWasNotExact,
        #[snafu(display("the Diesel database URL could not be created"))]
        DatabaseUrlCreationFailed,
        #[snafu(display("the Diesel database URL encoding changed"))]
        DatabaseUrlEncodingChanged,
        #[snafu(display("Diesel opened a missing read-write database"))]
        MissingDatabaseWasOpened,
        #[snafu(display("Diesel created a missing read-write database artifact"))]
        MissingDatabaseArtifactWasCreated,
        #[snafu(display("the database error was not redacted"))]
        DatabaseErrorWasNotRedacted,
        #[snafu(display("the migration database path was not UTF-8"))]
        DatabasePathWasNotUtf8,
        #[snafu(display("the immediate-transaction probe failed"))]
        TransactionProbeFailed,
        #[snafu(display("BEGIN IMMEDIATE did not reserve the SQLite write lock"))]
        ImmediateTransactionDidNotReserveWrite,
    }

    struct DatabaseFixture {
        path: PathBuf,
    }

    impl DatabaseFixture {
        fn new() -> Self {
            Self {
                path: std::env::temp_dir().join(format!(
                    "natsume-server-missing-database-test-{}.sqlite3",
                    Uuid::now_v7()
                )),
            }
        }

        fn wal_path(&self) -> PathBuf {
            PathBuf::from(format!("{}-wal", self.path.display()))
        }

        fn shm_path(&self) -> PathBuf {
            PathBuf::from(format!("{}-shm", self.path.display()))
        }
    }

    impl Drop for DatabaseFixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(self.wal_path());
            let _ = fs::remove_file(self.shm_path());
        }
    }
}
