use diesel::{
    connection::SimpleConnection,
    prelude::*,
    r2d2::{ConnectionManager, Pool},
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use once_cell::sync::OnceCell;

pub(crate) const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");

pub static DB_CONNECTION_POOL: OnceCell<Pool<ConnectionManager<SqliteConnection>>> =
    OnceCell::new();

#[derive(Debug)]
pub struct ConnectionOptions;

impl diesel::r2d2::CustomizeConnection<SqliteConnection, diesel::r2d2::Error>
    for ConnectionOptions
{
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        (|| {
            conn.batch_execute("PRAGMA busy_timeout = 5000;")?;
            conn.batch_execute(r#"
PRAGMA journal_mode = WAL;          -- better write-concurrency
PRAGMA synchronous = NORMAL;        -- fsync only in critical moments
PRAGMA wal_autocheckpoint = 1000;   -- write WAL changes back every 1000 pages, for an in average 1MB WAL file. May affect readers if number is increased
PRAGMA foreign_keys = ON;           -- enforce foreign keys
            "#)?;
            Ok(())
        })()
        .map_err(diesel::r2d2::Error::QueryError)
    }
}

fn run_migrations(
    connection: &mut impl MigrationHarness<diesel::sqlite::Sqlite>,
) -> anyhow::Result<()> {
    connection
        .run_pending_migrations(MIGRATIONS)
        .map_err(|e| anyhow::Error::msg(e.to_string()))?;

    Ok(())
}

pub fn get_connection_pool() -> Pool<ConnectionManager<SqliteConnection>> {
    let manager = ConnectionManager::<SqliteConnection>::new("database.db");
    Pool::builder()
        .connection_customizer(Box::new(ConnectionOptions))
        .test_on_check_out(true)
        .build(manager)
        .expect("Could not build connection pool")
}

pub fn init_database() -> anyhow::Result<()> {
    let connection_pool: Pool<ConnectionManager<SqliteConnection>> = get_connection_pool();

    let mut connection = connection_pool.get()?;
    run_migrations(&mut connection)?;

    DB_CONNECTION_POOL
        .set(connection_pool)
        .map_err(|_| anyhow::Error::msg("DB_CONNECTION_POOL already inited!"))?;
    Ok(())
}
