use super::{
    api::Api,
    error::Result,
    state::Store,
    wire::{Binding, Pairing},
};

/// Platform credential operations; no token is persisted in the execution journal.
pub(super) trait Credentials {
    fn load(&self, origin: &str, client_id: &str) -> Result<String>;
    fn save(&self, origin: &str, client_id: &str, token: &str) -> Result<()>;
    fn remove(&self, origin: &str, client_id: &str) -> Result<()>;
}

pub(super) fn restore(store: &mut Store, credentials: &impl Credentials) -> Result<Option<String>> {
    let Some(binding) = store.state.binding.clone() else {
        return Ok(None);
    };
    if binding.confirmed {
        let token = credentials.load(&store.state.server_url, &binding.client_id)?;
        Pairing {
            printer_id: binding.printer_id,
            client_id: binding.client_id,
            token: token.clone(),
        }
        .validate()?;
        return Ok(Some(token));
    }
    credentials.remove(&store.state.server_url, &binding.client_id)?;
    let mut state = store.state.clone();
    state.binding = None;
    state.paused = true;
    store.save(state)?;
    tracing::warn!(
        "unfinished pairing was discarded; an administrator must clear the old binding and issue a new code"
    );
    Ok(None)
}

pub(super) async fn complete(
    store: &mut Store,
    mut api: Api,
    credentials: &impl Credentials,
    code: &str,
) -> Result<Api> {
    let pairing = api.pair(code).await?;
    let binding = Binding {
        printer_id: pairing.printer_id,
        client_id: pairing.client_id,
        confirmed: false,
    };
    let mut state = store.state.clone();
    state.binding = Some(binding.clone());
    state.paused = true;
    store.save(state)?;
    let result = async {
        credentials.save(&store.state.server_url, &binding.client_id, &pairing.token)?;
        api.authenticate(pairing.token);
        api.confirm().await?.validate(&binding)?;
        let mut state = store.state.clone();
        state.binding = Some(Binding {
            confirmed: true,
            ..binding.clone()
        });
        store.save(state)?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        tracing::error!(
            "pairing did not complete; administrator cleanup and a fresh code are required"
        );
        credentials.remove(&store.state.server_url, &binding.client_id)?;
        return Err(error);
    }
    Ok(api)
}
