use std::path::PathBuf;

use diesel::{ExpressionMethods, RunQueryDsl};
use uuid::Uuid;

use crate::{
    db::{Database, DatabaseConfig, PersistenceError},
    diesel_schema::runtime_config,
};

use super::{RuntimeConfigComponent, RuntimeConfigError, is_canonical_https_origin};

#[tokio::test]
async fn accepts_only_exact_canonical_https_origins() {
    let fixture = Fixture::new().await;
    for origin in [
        "https://judge.example.test",
        "https://judge.example.test:8443",
        "https://192.0.2.10",
        "https://[2001:db8::1]:8443",
    ] {
        assert!(is_canonical_https_origin(origin));
        fixture.persist(1, origin).await;
        assert_eq!(fixture.materialize().await, origin);
        assert_eq!(
            fixture.component.read_current().await,
            Ok(Some(origin.to_owned()))
        );
    }
}

#[test]
fn rejects_invalid_or_noncanonical_origins() {
    for origin in [
        "http://judge.example.test",
        "https://user@judge.example.test",
        "https://user:password@judge.example.test",
        "https://judge.example.test/login",
        "https://judge.example.test?contest=1",
        "https://judge.example.test#fragment",
        "https://judge.example.test/",
        "https://JUDGE.example.test",
        "https://judge.example.test:443",
        "not a URL",
    ] {
        assert!(!is_canonical_https_origin(origin));
    }
}

#[tokio::test]
async fn missing_config_fails_closed() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.component.read_current().await, Ok(None));
    assert_eq!(
        fixture.component.materialize().await,
        Err(RuntimeConfigError::MissingConfiguration)
    );
}

#[tokio::test]
async fn replacement_survives_component_rebuild() {
    let fixture = Fixture::new().await;
    fixture.persist(1, "https://first.example.test").await;
    fixture.persist(1, "https://second.example.test:8443").await;

    let rebuilt = RuntimeConfigComponent::new(fixture.database.clone());
    assert_eq!(
        rebuilt
            .materialize()
            .await
            .unwrap_or_else(|error| panic!("rebuilt materialization failed: {error}")),
        "https://second.example.test:8443"
    );
}

#[tokio::test]
async fn invalid_persisted_fact_fails_closed() {
    let fixture = Fixture::new().await;
    fixture.persist(1, "http://judge.example.test").await;
    assert_eq!(
        fixture.component.materialize().await,
        Err(RuntimeConfigError::InvalidPersistedFacts)
    );
    assert_eq!(
        fixture.component.read_current().await,
        Err(RuntimeConfigError::InvalidPersistedFacts)
    );
}

struct Fixture {
    path: PathBuf,
    database: Database,
    component: RuntimeConfigComponent,
}

impl Fixture {
    async fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "natsume-runtime-component-test-{}.sqlite3",
            Uuid::now_v7()
        ));
        let database = Database::connect_and_migrate(&DatabaseConfig::new(&path, true))
            .await
            .unwrap_or_else(|error| panic!("Runtime Config test database failed: {error:?}"));
        let component = RuntimeConfigComponent::new(database.clone());
        Self {
            path,
            database,
            component,
        }
    }

    async fn materialize(&self) -> String {
        self.component
            .materialize()
            .await
            .unwrap_or_else(|error| panic!("Runtime Config materialization failed: {error}"))
    }

    async fn persist(&self, singleton: i32, origin: &'static str) {
        self.database
            .write(move |transaction| {
                diesel::replace_into(runtime_config::table)
                    .values((
                        runtime_config::singleton.eq(singleton),
                        runtime_config::domjudge_origin.eq(origin),
                    ))
                    .execute(transaction.connection())
                    .map_err(|_| PersistenceError::OperationFailed)?;
                Ok::<(), PersistenceError>(())
            })
            .await
            .unwrap_or_else(|error| panic!("persisted Runtime Config fixture failed: {error:?}"));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
