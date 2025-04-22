use diesel::{
    prelude::*,
    r2d2::{ConnectionManager, Pool},
};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use once_cell::sync::OnceCell;

pub(crate) const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");

pub static DB_CONNECTION_POOL: OnceCell<Pool<ConnectionManager<SqliteConnection>>> = OnceCell::new();

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
