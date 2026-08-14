use std::{path::PathBuf, time::Duration};

use diesel::{
    Connection, RunQueryDsl,
    connection::{CacheSize, SimpleConnection},
    r2d2::{ConnectionManager, CustomizeConnection, NopErrorHandler, Pool},
    sql_types::{Integer, Text},
    sqlite::SqliteConnection,
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use snafu::Snafu;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
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

pub(crate) mod contest;
pub(crate) mod operator;
pub mod provisioning;
pub(crate) mod schema;

pub struct DatabaseConfig {
    database_path: PathBuf,
    create_if_missing: bool,
}

impl DatabaseConfig {
    #[must_use]
    pub fn new(database_path: impl Into<PathBuf>, create_if_missing: bool) -> Self {
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
pub struct Database {
    pool: Pool<ConnectionManager<SqliteConnection>>,
}

impl Database {
    /// Connects to `SQLite` and applies embedded migrations.
    ///
    /// # Errors
    ///
    /// Returns a redacted [`DatabaseError`] when configuration, connection,
    /// or migration fails.
    pub async fn connect_and_migrate(config: &DatabaseConfig) -> Result<Self, DatabaseError> {
        if !config.is_valid() {
            return Err(DatabaseError::InvalidConfiguration);
        }

        let pool = build_pool(config)?;
        let migration_pool = pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = migration_pool
                .get()
                .map_err(|_| DatabaseError::ConnectionFailed)?;
            connection
                .run_pending_migrations(MIGRATIONS)
                .map_err(|_| DatabaseError::MigrationFailed)?;
            Ok::<(), DatabaseError>(())
        })
        .await
        .map_err(|_| DatabaseError::ConnectionFailed)??;
        Ok(Self { pool })
    }

    pub(crate) async fn interact<T, F>(&self, operation: F) -> Result<T, DatabaseError>
    where
        T: Send + 'static,
        F: FnOnce(&mut SqliteConnection) -> T + Send + 'static,
    {
        let pool = self.pool.clone();
        // A blocking thread does not inherit a scoped dispatcher, so without
        // this every event emitted inside `operation` would be dropped. In
        // production the captured dispatcher is the installed global one.
        let dispatcher = tracing::dispatcher::get_default(Clone::clone);
        tokio::task::spawn_blocking(move || {
            tracing::dispatcher::with_default(&dispatcher, || {
                let mut connection = pool.get().map_err(|_| DatabaseError::ConnectionFailed)?;
                Ok(operation(&mut connection))
            })
        })
        .await
        .map_err(|_| DatabaseError::ConnectionFailed)?
    }
}

fn build_pool(
    config: &DatabaseConfig,
) -> Result<Pool<ConnectionManager<SqliteConnection>>, DatabaseError> {
    let manager = ConnectionManager::new(diesel_database_url(config)?);
    Pool::builder()
        .max_size(10)
        .min_idle(Some(0))
        .connection_timeout(Duration::from_secs(30))
        .idle_timeout(Some(Duration::from_mins(10)))
        .max_lifetime(Some(Duration::from_mins(30)))
        .test_on_check_out(true)
        .error_handler(Box::new(NopErrorHandler))
        .connection_customizer(Box::new(SqliteConnectionCustomizer))
        .build(manager)
        .map_err(|_| DatabaseError::ConnectionFailed)
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
        Ok(())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Snafu)]
pub enum DatabaseError {
    #[snafu(display("the database configuration is invalid"))]
    InvalidConfiguration,
    #[snafu(display("the database connection failed"))]
    ConnectionFailed,
    #[snafu(display("the database migration failed"))]
    MigrationFailed,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };

    use diesel::{
        Connection, QueryableByName, RunQueryDsl, connection::SimpleConnection, sql_types::BigInt,
        sqlite::SqliteConnection,
    };
    use diesel_migrations::{FileBasedMigrations, MigrationHarness};
    use snafu::Snafu;
    use uuid::Uuid;

    use super::{
        Database, DatabaseConfig, DatabaseError, diesel_database_url, sqlite_pragma_values,
    };

    /// Opens an independent connection outside the pool so tests can observe
    /// committed state without joining the pool's own contention.
    pub(crate) fn test_observer(path: &Path) -> Result<SqliteConnection, DatabaseError> {
        let path = path.to_str().ok_or(DatabaseError::InvalidConfiguration)?;
        SqliteConnection::establish(path).map_err(|_| DatabaseError::ConnectionFailed)
    }

    /// Holds the `SQLite` write lock for as long as the returned connection
    /// lives, so tests can prove which paths need it.
    pub(crate) fn test_lock_database(path: &Path) -> Result<SqliteConnection, DatabaseError> {
        let mut connection = test_observer(path)?;
        connection
            .batch_execute("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .map_err(|_| DatabaseError::ConnectionFailed)?;
        Ok(connection)
    }

    /// Reads the observer's `PRAGMA data_version`, which advances only when
    /// another connection commits a write.
    pub(crate) fn test_data_version(
        connection: &mut SqliteConnection,
    ) -> Result<i64, DatabaseError> {
        diesel::dsl::sql::<BigInt>("PRAGMA data_version")
            .get_result(connection)
            .map_err(|_| DatabaseError::ConnectionFailed)
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

        assert_eq!(error, DatabaseError::ConnectionFailed);
        for path in [&fixture.path, &fixture.wal_path(), &fixture.shm_path()] {
            assert!(!path.exists(), "serve-mode database artifacts were created");
        }

        let display = error.to_string();
        let debug = format!("{error:?}");
        let canary = fixture.path.to_string_lossy();
        assert!(!display.contains(canary.as_ref()));
        assert!(!debug.contains(canary.as_ref()));
    }

    #[tokio::test]
    async fn diesel_pool_connections_have_exact_pragmas() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&fixture.path, true))
            .await
            .map_err(|_| TestFailure::DatabaseCreationFailed)?;
        let pragmas = database
            .interact(sqlite_pragma_values)
            .await
            .map_err(|_| TestFailure::DieselInteractionFailed)?
            .map_err(|_| TestFailure::PragmasWereNotReadable)?;
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
            .map_err(|_| TestFailure::DatabaseCreationFailed)?;
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

    #[test]
    fn diesel_read_write_mode_does_not_create_a_missing_database() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let config = DatabaseConfig::new(&fixture.path, false);
        let database_url =
            diesel_database_url(&config).map_err(|_| TestFailure::DatabaseUrlCreationFailed)?;
        if let Ok(connection) = diesel::sqlite::SqliteConnection::establish(&database_url) {
            drop(connection);
            return Err(TestFailure::MissingDatabaseWasOpened);
        }
        for path in [&fixture.path, &fixture.wal_path(), &fixture.shm_path()] {
            if path.exists() {
                return Err(TestFailure::MissingDatabaseArtifactWasCreated);
            }
        }
        let error = DatabaseError::ConnectionFailed;
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
            .map_err(|_| TestFailure::DatabaseUrlCreationFailed)?;
        let create = diesel_database_url(&DatabaseConfig::new(&path, true))
            .map_err(|_| TestFailure::DatabaseUrlCreationFailed)?;
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
            .map_err(|_| TestFailure::TransactionProbeFailed)?;
        let mut competitor = diesel::sqlite::SqliteConnection::establish(path)
            .map_err(|_| TestFailure::TransactionProbeFailed)?;
        competitor
            .batch_execute("PRAGMA busy_timeout = 0;")
            .map_err(|_| TestFailure::TransactionProbeFailed)?;

        owner.immediate_transaction(|_connection| {
            if competitor.batch_execute("BEGIN IMMEDIATE;").is_ok() {
                competitor
                    .batch_execute("ROLLBACK;")
                    .map_err(|_| TestFailure::TransactionProbeFailed)?;
                return Err(TestFailure::ImmediateTransactionDidNotReserveWrite);
            }
            Ok(())
        })
    }

    #[test]
    fn diesel_up_migration_has_exact_business_table_contract() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let (mut connection, migrations) = migration_fixture(&fixture)?;
        connection
            .run_pending_migrations(migrations)
            .map_err(|_| TestFailure::MigrationUpFailed)?;
        let tables = business_tables(&mut connection)?;
        let expected = [
            "account_mappings",
            "accounts",
            "audit_events",
            "commands",
            "device_bindings",
            "device_tokens",
            "devices",
            "enrollment_requests",
            "gateway_certificates",
            "observed_device_states",
            "operator_accounts",
            "operator_sessions",
            "pending_import_candidate",
            "provisioning_window",
            "revision_counters",
            "seats",
            "server_vault_records",
            "site_identity",
        ];
        if !tables.iter().map(|table| table.name.as_str()).eq(expected) {
            return Err(TestFailure::BusinessTablesWereNotExact);
        }
        if tables.iter().any(|table| table.strict != 1) {
            return Err(TestFailure::StrictContractWasNotExact);
        }
        let trigger_count = diesel::sql_query(
            "SELECT COUNT(*) AS trigger_count FROM sqlite_schema WHERE type = 'trigger'",
        )
        .get_result::<TriggerCount>(&mut connection)
        .map_err(|_| TestFailure::TriggerContractWasNotReadable)?
        .trigger_count;
        if trigger_count != 0 {
            return Err(TestFailure::TriggerContractWasNotExact);
        }
        Ok(())
    }

    #[test]
    fn diesel_down_then_up_succeeds() -> Result<(), TestFailure> {
        let fixture = DatabaseFixture::new();
        let (mut connection, migrations) = migration_fixture(&fixture)?;
        let first_up_count = connection
            .run_pending_migrations(migrations.clone())
            .map_err(|_| TestFailure::MigrationUpFailed)?
            .len();
        let down_count = connection
            .revert_all_migrations(migrations.clone())
            .map_err(|_| TestFailure::MigrationDownFailed)?
            .len();
        let second_up_count = connection
            .run_pending_migrations(migrations)
            .map_err(|_| TestFailure::MigrationUpFailed)?
            .len();
        if first_up_count != 1
            || down_count != 1
            || second_up_count != 1
            || business_tables(&mut connection)?.len() != 18
        {
            return Err(TestFailure::MigrationRoundTripWasNotExact);
        }
        Ok(())
    }

    fn migration_fixture(
        fixture: &DatabaseFixture,
    ) -> Result<(diesel::sqlite::SqliteConnection, FileBasedMigrations), TestFailure> {
        let path = fixture
            .path
            .to_str()
            .ok_or(TestFailure::DatabasePathWasNotUtf8)?;
        let mut connection = diesel::sqlite::SqliteConnection::establish(path)
            .map_err(|_| TestFailure::MigrationDatabaseCreationFailed)?;
        connection
            .batch_execute("PRAGMA foreign_keys = ON;")
            .map_err(|_| TestFailure::MigrationDatabaseCreationFailed)?;
        let migrations = FileBasedMigrations::from_path(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"),
        )
        .map_err(|_| TestFailure::MigrationSourceWasNotReadable)?;
        Ok((connection, migrations))
    }

    fn business_tables(
        connection: &mut diesel::sqlite::SqliteConnection,
    ) -> Result<Vec<SchemaTable>, TestFailure> {
        diesel::sql_query(
            "SELECT name, strict FROM pragma_table_list WHERE schema = 'main' \
             AND name NOT LIKE 'sqlite_%' AND name <> '__diesel_schema_migrations' ORDER BY name",
        )
        .load(connection)
        .map_err(|_| TestFailure::BusinessTablesWereNotReadable)
    }

    #[derive(QueryableByName)]
    struct SchemaTable {
        #[diesel(sql_type = diesel::sql_types::Text)]
        name: String,
        #[diesel(sql_type = diesel::sql_types::Integer)]
        strict: i32,
    }

    #[derive(QueryableByName)]
    struct TriggerCount {
        #[diesel(sql_type = BigInt)]
        trigger_count: i64,
    }

    impl From<diesel::result::Error> for TestFailure {
        fn from(_source: diesel::result::Error) -> Self {
            Self::TransactionProbeFailed
        }
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the test database could not be created"))]
        DatabaseCreationFailed,
        #[snafu(display("the Diesel interaction failed"))]
        DieselInteractionFailed,
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
        #[snafu(display("the migration database could not be created"))]
        MigrationDatabaseCreationFailed,
        #[snafu(display("the Diesel migration source was not readable"))]
        MigrationSourceWasNotReadable,
        #[snafu(display("the Diesel up migration failed"))]
        MigrationUpFailed,
        #[snafu(display("the Diesel down migration failed"))]
        MigrationDownFailed,
        #[snafu(display("the Diesel business tables were not readable"))]
        BusinessTablesWereNotReadable,
        #[snafu(display("the Diesel business table set was not exact"))]
        BusinessTablesWereNotExact,
        #[snafu(display("the Diesel STRICT table contract was not exact"))]
        StrictContractWasNotExact,
        #[snafu(display("the Diesel trigger contract was not readable"))]
        TriggerContractWasNotReadable,
        #[snafu(display("the Diesel trigger contract was not exact"))]
        TriggerContractWasNotExact,
        #[snafu(display("the Diesel migration round trip was not exact"))]
        MigrationRoundTripWasNotExact,
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
