use crate::{
    component::{
        operator::{OperatorComponent, OperatorCredentials},
        runtime::RuntimeConfigComponent,
    },
    config::ServerConfig,
    db::{Database, DatabaseConfig},
    vault::ensure_master_key,
};

use super::{CommandError, credentials};

pub(super) async fn execute(config: ServerConfig) -> Result<(), CommandError> {
    bootstrap_with(config, read_credentials_from_tty).await?;
    tracing::info!("bootstrap completed");
    Ok(())
}

async fn bootstrap_with<F>(config: ServerConfig, read_credentials: F) -> Result<(), CommandError>
where
    F: FnOnce() -> Result<OperatorCredentials, CommandError>,
{
    let database_config = DatabaseConfig::new(config.database_path(), true);
    let database = Database::connect_and_migrate(&database_config)
        .await
        .map_err(|_| CommandError::Database)?;
    tracing::info!("database ready");
    ensure_master_key(config.vault_master_key_path()).map_err(|_| CommandError::Vault)?;
    tracing::info!("vault key verified");
    let credentials = read_credentials()?;
    let password_hash = credentials
        .hash_password()
        .map_err(|_| CommandError::Bootstrap)?;
    database
        .write(move |transaction| {
            OperatorComponent::create_first_admin(
                transaction,
                credentials.login_name(),
                &password_hash,
            )
            .map_err(|_| CommandError::Bootstrap)?;
            RuntimeConfigComponent::apply_deployment_config(transaction, config.domjudge_origin())
                .map_err(|_| CommandError::Bootstrap)
        })
        .await
        .map_err(|_| CommandError::Bootstrap)?;
    Ok(())
}

fn read_credentials_from_tty() -> Result<OperatorCredentials, CommandError> {
    credentials::read_from_tty(CommandError::Bootstrap)
}

#[cfg(test)]
pub(super) mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    use diesel::connection::SimpleConnection;
    use uuid::Uuid;

    use crate::{
        component::{
            operator::{
                OperatorComponent, OperatorCredentials, tests::PasswordVerificationTestGuard,
            },
            runtime::RuntimeConfigComponent,
        },
        config::ServerConfig,
        db::{Database, DatabaseConfig, PersistenceError},
    };

    use super::{CommandError, bootstrap_with};

    #[tokio::test]
    async fn bootstrap_initializes_admin_and_runtime_configuration() {
        let _guard = PasswordVerificationTestGuard::acquire().await;
        let fixture = Fixture::new();
        fixture
            .bootstrap("admin")
            .await
            .unwrap_or_else(|error| panic!("bootstrap failed: {error}"));
        let database = fixture.database(false).await;
        let session = OperatorComponent::new(database.clone())
            .sign_in("admin", "test-password".to_owned())
            .await
            .unwrap_or_else(|error| panic!("bootstrap administrator cannot sign in: {error}"));
        assert_eq!(session.identity().role_name(), "admin");
        assert_eq!(
            RuntimeConfigComponent::new(database).materialize().await,
            Ok("https://judge.example.test".to_owned())
        );
    }

    #[tokio::test]
    async fn repeated_bootstrap_preserves_existing_state() {
        let fixture = Fixture::new();
        fixture
            .bootstrap("admin")
            .await
            .unwrap_or_else(|error| panic!("bootstrap failed: {error}"));
        let key_path = fixture.directory.join("keys/server-root.key");
        let key = fs::read(&key_path).unwrap_or_else(|error| panic!("read vault key: {error}"));
        fixture.set_origin("https://other.example.test");
        assert_eq!(
            fixture.bootstrap("second-admin").await,
            Err(CommandError::Bootstrap)
        );
        assert_eq!(
            fs::read(&key_path).unwrap_or_else(|error| panic!("reread vault key: {error}")),
            key
        );
        let database = fixture.database(false).await;
        assert_eq!(
            RuntimeConfigComponent::new(database.clone())
                .materialize()
                .await,
            Ok("https://judge.example.test".to_owned())
        );
        let accounts = database
            .read(|transaction| {
                use crate::diesel_schema::operator_accounts;
                use diesel::{QueryDsl, RunQueryDsl};
                operator_accounts::table
                    .select(operator_accounts::username)
                    .load::<String>(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .unwrap_or_else(|error| panic!("read administrators: {error:?}"));
        assert_eq!(accounts, ["admin"]);
    }

    #[tokio::test]
    async fn runtime_failure_rolls_back_admin_and_allows_retry() {
        let fixture = Fixture::new();
        let database = fixture.database(true).await;
        fixture.sql("CREATE TRIGGER fail_runtime BEFORE INSERT ON runtime_config BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").await;
        assert_eq!(
            fixture.bootstrap("admin").await,
            Err(CommandError::Bootstrap)
        );
        let initialized = database
            .read(OperatorComponent::is_initialized)
            .await
            .unwrap_or_else(|error| panic!("read bootstrap status: {error:?}"));
        assert!(!initialized);
        assert_eq!(
            RuntimeConfigComponent::new(database.clone())
                .read_current()
                .await,
            Ok(None)
        );
        fixture.sql("DROP TRIGGER fail_runtime;").await;
        fixture
            .bootstrap("admin")
            .await
            .unwrap_or_else(|error| panic!("bootstrap retry failed: {error}"));
        assert_eq!(
            RuntimeConfigComponent::new(database).materialize().await,
            Ok("https://judge.example.test".to_owned())
        );
    }

    pub(in crate::commands) struct Fixture {
        directory: PathBuf,
    }

    impl Fixture {
        pub(in crate::commands) fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("natsume-bootstrap-{}", Uuid::now_v7()));
            fs::create_dir_all(directory.join("keys"))
                .unwrap_or_else(|error| panic!("create bootstrap fixture: {error}"));
            fs::set_permissions(directory.join("keys"), fs::Permissions::from_mode(0o700))
                .unwrap_or_else(|error| panic!("protect bootstrap keys: {error}"));
            let config = format!(
                r#"
[listen]
https = "127.0.0.1:8443"
[storage]
database = "{root}/natsume.db"
root_key = "{root}/keys/server-root.key"
organization_logos = "/var/lib/natsume-server/organization-logos"
[tls]
certificate = "{root}/keys/server-tls-leaf.der"
private_key = "{root}/keys/server-tls-key.pk8"
[site]
gateway_hostname = "gateway.example.test"
gateway_not_after = "2099-01-02T00:00:00Z"
contest_end = "2099-01-01T00:00:00Z"
[trust]
control_root = "{root}/control-ca.crt"
local_origin_root = "{root}/local-origin-ca.crt"
[runtime]
domjudge_origin = "https://judge.example.test"
"#,
                root = directory.display()
            );
            fs::write(directory.join("config.toml"), config)
                .unwrap_or_else(|error| panic!("write bootstrap config: {error}"));
            Self { directory }
        }

        fn config(&self) -> ServerConfig {
            ServerConfig::load_from(&self.directory.join("config.toml"))
                .unwrap_or_else(|error| panic!("load bootstrap config: {error}"))
        }

        fn set_origin(&self, origin: &str) {
            let path = self.directory.join("config.toml");
            let config = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read bootstrap config: {error}"));
            fs::write(path, config.replace("https://judge.example.test", origin))
                .unwrap_or_else(|error| panic!("change bootstrap config: {error}"));
        }

        pub(in crate::commands) async fn bootstrap(&self, login: &str) -> Result<(), CommandError> {
            bootstrap_with(self.config(), || {
                OperatorCredentials::new(
                    login.to_owned(),
                    "test-password".to_owned(),
                    "test-password".to_owned(),
                )
                .map_err(|_| CommandError::Bootstrap)
            })
            .await
        }

        pub(in crate::commands) async fn sql(&self, sql: &'static str) {
            self.database(false)
                .await
                .write(move |transaction| {
                    transaction
                        .connection()
                        .batch_execute(sql)
                        .map_err(|_| PersistenceError::OperationFailed)
                })
                .await
                .unwrap_or_else(|error| panic!("apply database fixture: {error:?}"));
        }

        pub(in crate::commands) async fn database(&self, create_if_missing: bool) -> Database {
            Database::connect_and_migrate(&DatabaseConfig::new(
                self.directory.join("natsume.db"),
                create_if_missing,
            ))
            .await
            .unwrap_or_else(|error| panic!("open bootstrap database: {error}"))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}
