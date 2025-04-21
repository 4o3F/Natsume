use diesel_migrations::{embed_migrations, EmbeddedMigrations};

pub(crate) const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");


