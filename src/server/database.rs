use std::time::Duration;

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
pub struct ConnectionOptions {
    pub enable_wal: bool,
    pub enable_foreign_keys: bool,
    pub busy_timeout: Option<Duration>,
}

impl diesel::r2d2::CustomizeConnection<SqliteConnection, diesel::r2d2::Error>
    for ConnectionOptions
{
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        (|| {
            if self.enable_wal {
                conn.batch_execute("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
            }
            if self.enable_foreign_keys {
                conn.batch_execute("PRAGMA foreign_keys = ON;")?;
            }
            if let Some(d) = self.busy_timeout {
                conn.batch_execute(&format!("PRAGMA busy_timeout = {};", d.as_millis()))?;
            }
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
        .connection_customizer(Box::new(ConnectionOptions {
            enable_wal: true,
            enable_foreign_keys: true,
            busy_timeout: Some(Duration::from_secs(30)),
        }))
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
