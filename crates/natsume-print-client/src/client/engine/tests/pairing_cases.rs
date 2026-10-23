use super::*;
use crate::client::pairing::{self, Credentials};

struct FakeCredentials {
    token: Mutex<Option<String>>,
    fail_save: bool,
    fail_remove: bool,
    journal: std::path::PathBuf,
}

impl FakeCredentials {
    fn new(config: &Config) -> Self {
        Self {
            token: Mutex::new(None),
            fail_save: false,
            fail_remove: false,
            journal: config.state_directory.join("state.jsonl"),
        }
    }
}

impl Credentials for FakeCredentials {
    fn load(&self, _: &str, client_id: &str) -> Result<String> {
        assert_eq!(client_id, CLIENT);
        must(self.token.lock()).clone().ok_or_else(|| {
            AttentionSnafu {
                reason: "missing fixture credential",
            }
            .build()
        })
    }

    fn save(&self, _: &str, client_id: &str, token: &str) -> Result<()> {
        assert_eq!(client_id, CLIENT);
        let journal = must(std::fs::read_to_string(&self.journal));
        let record: Value = must(serde_json::from_str(
            journal.lines().last().unwrap_or_default(),
        ));
        assert_eq!(record["state"]["binding"]["confirmed"], false);
        assert_eq!(record["state"]["binding"]["client_id"], CLIENT);
        *must(self.token.lock()) = Some(token.to_owned());
        if self.fail_save {
            return AttentionSnafu {
                reason: "injected credential write failure",
            }
            .fail();
        }
        Ok(())
    }

    fn remove(&self, _: &str, client_id: &str) -> Result<()> {
        assert_eq!(client_id, CLIENT);
        if self.fail_remove {
            return AttentionSnafu {
                reason: "injected credential removal failure",
            }
            .fail();
        }
        *must(self.token.lock()) = None;
        Ok(())
    }
}

fn pair_reply() -> Reply {
    Reply::Json(
        json!({"printer_id":PRINTER, "client_id":CLIENT, "token":format!("npc1.{CLIENT}.{}", "a".repeat(64))}),
    )
}

fn confirm_reply(client: &str) -> Reply {
    Reply::Json(
        json!({"printer_id":PRINTER, "client_id":client, "name":"Printer", "client_confirmed":true, "pairing_expires_at_unix_ms":null}),
    )
}

#[tokio::test]
async fn complete_pairing_persists_confirmation_without_writing_secrets_to_the_journal() {
    let server = https(vec![pair_reply(), confirm_reply(CLIENT)]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let credentials = FakeCredentials::new(&config);
    let _api = must(
        pairing::complete(
            &mut store,
            must(Api::new(&config)),
            &credentials,
            "ABCD-2345",
        )
        .await,
    );
    assert!(store.state.paused);
    assert_eq!(store.state.binding, Some(binding()));
    let journal = must(std::fs::read_to_string(&credentials.journal));
    assert!(!journal.contains("npc1."));
    drop(store);
    let mut store = must(Store::open(&config));
    assert!(must(pairing::restore(&mut store, &credentials)).is_some());
    assert_eq!(must(server.seen.lock()).len(), 2);
}

#[tokio::test]
async fn lost_confirmation_discards_credentials_and_never_automatically_confirms_again() {
    let server = https(vec![pair_reply(), Reply::Lost]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let credentials = FakeCredentials::new(&config);
    assert!(
        pairing::complete(
            &mut store,
            must(Api::new(&config)),
            &credentials,
            "ABCD-2345"
        )
        .await
        .is_err()
    );
    assert!(must(credentials.token.lock()).is_none());
    assert!(
        store
            .state
            .binding
            .as_ref()
            .is_some_and(|binding| !binding.confirmed)
    );
    drop(store);
    let mut store = must(Store::open(&config));
    assert!(must(pairing::restore(&mut store, &credentials)).is_none());
    assert!(store.state.binding.is_none());
    assert!(store.state.paused);
    assert_eq!(must(server.seen.lock()).len(), 2);
}

#[tokio::test]
async fn an_inconsistent_confirmation_does_not_create_a_confirmed_binding() {
    let server = https(vec![pair_reply(), confirm_reply(PRINTER)]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let credentials = FakeCredentials::new(&config);
    assert!(
        pairing::complete(
            &mut store,
            must(Api::new(&config)),
            &credentials,
            "ABCD-2345"
        )
        .await
        .is_err()
    );
    assert!(must(credentials.token.lock()).is_none());
    assert!(
        store
            .state
            .binding
            .as_ref()
            .is_some_and(|binding| !binding.confirmed)
    );
}

#[tokio::test]
async fn credential_save_failure_is_cleaned_even_when_the_secret_was_written() {
    let server = https(vec![pair_reply()]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let mut credentials = FakeCredentials::new(&config);
    credentials.fail_save = true;
    assert!(
        pairing::complete(
            &mut store,
            must(Api::new(&config)),
            &credentials,
            "ABCD-2345"
        )
        .await
        .is_err()
    );
    assert!(must(credentials.token.lock()).is_none());
    assert_eq!(must(server.seen.lock()).len(), 1);
}

#[tokio::test]
async fn a_cleanup_failure_keeps_pairing_unconfirmed_and_blocks_execution() {
    let server = https(vec![pair_reply(), Reply::Lost]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let mut credentials = FakeCredentials::new(&config);
    credentials.fail_remove = true;
    assert!(
        pairing::complete(
            &mut store,
            must(Api::new(&config)),
            &credentials,
            "ABCD-2345"
        )
        .await
        .is_err()
    );
    assert!(must(credentials.token.lock()).is_some());
    assert!(pairing::restore(&mut store, &credentials).is_err());
    assert!(Engine::new(store, api(&config), FakePrinter::default()).is_err());
}

#[tokio::test]
async fn missing_confirmed_credentials_do_not_silently_clear_the_binding() {
    let server = https(Vec::new()).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut store = must(Store::open(&config));
    let mut state = store.state.clone();
    state.binding = Some(binding());
    must(store.save(state));
    assert!(pairing::restore(&mut store, &FakeCredentials::new(&config)).is_err());
    assert_eq!(store.state.binding, Some(binding()));
    assert!(must(server.seen.lock()).is_empty());
}
