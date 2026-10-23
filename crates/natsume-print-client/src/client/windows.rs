use std::{io::BufRead as _, path::Path, time::Duration};

use ::windows::Security::Credentials::{PasswordCredential, PasswordVault};
use snafu::{ResultExt as _, ensure};
use tokio::sync::mpsc;

use super::{
    api::Api,
    config::Config,
    engine::{Engine, Printer, locked},
    error::{AttentionSnafu, ClientError, CredentialSnafu, Result, StorageSnafu},
    pairing::{self, Credentials},
    state::Store,
};
use crate::{Document, JobObservation};

struct NativePrinter;
impl Printer for NativePrinter {
    fn ready(&self, queue: &str) -> crate::Result<bool> {
        Ok(crate::backend::inspect_queue(queue)?.can_submit())
    }
    fn observe(&self, queue: &str, job: u32, label: &str) -> crate::Result<JobObservation> {
        crate::backend::observe_job(queue, job, label)
    }
    fn submit(
        &self,
        queue: &str,
        document: &Document,
        started: impl FnOnce(u32) -> crate::Result<()>,
    ) -> crate::Result<u32> {
        crate::backend::submit(queue, document, started)
    }
}

struct VaultCredentials;

fn resource(origin: &str) -> ::windows::core::HSTRING {
    format!("Natsume.Print/{origin}").into()
}

impl Credentials for VaultCredentials {
    fn load(&self, origin: &str, client_id: &str) -> Result<String> {
        let vault = PasswordVault::new().context(CredentialSnafu)?;
        let credential = vault
            .Retrieve(&resource(origin), &client_id.into())
            .context(CredentialSnafu)?;
        credential.RetrievePassword().context(CredentialSnafu)?;
        Ok(credential.Password().context(CredentialSnafu)?.to_string())
    }

    fn save(&self, origin: &str, client_id: &str, token: &str) -> Result<()> {
        let vault = PasswordVault::new().context(CredentialSnafu)?;
        let credential = PasswordCredential::CreatePasswordCredential(
            &resource(origin),
            &client_id.into(),
            &token.into(),
        )
        .context(CredentialSnafu)?;
        vault.Add(&credential).context(CredentialSnafu)
    }

    fn remove(&self, origin: &str, client_id: &str) -> Result<()> {
        let vault = PasswordVault::new().context(CredentialSnafu)?;
        match vault.Retrieve(&resource(origin), &client_id.into()) {
            Ok(credential) => vault.Remove(&credential).context(CredentialSnafu),
            Err(error) if error.code() == ::windows::core::HRESULT::from_win32(1168) => Ok(()),
            Err(source) => Err(source).context(CredentialSnafu),
        }
    }
}

async fn paired_api(store: &mut Store, mut api: Api) -> Result<Api> {
    if let Some(token) = pairing::restore(store, &VaultCredentials)? {
        api.authenticate(token);
        return Ok(api);
    }
    tracing::info!("enter a new pairing code; input is hidden");
    let code = tokio::task::spawn_blocking(rpassword::read_password)
        .await
        .context(super::error::WorkerSnafu)?
        .context(StorageSnafu {
            operation: "read pairing code",
        })?;
    pairing::complete(store, api, &VaultCredentials, code.trim()).await
}

#[derive(Clone, Copy)]
enum Control {
    Pause,
    Resume,
    Stop,
}

fn controls() -> Result<mpsc::Receiver<Control>> {
    let (send, receive) = mpsc::channel(8);
    std::thread::Builder::new()
        .name("print-client-input".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let control = match line {
                    Ok(line) => match line.trim() {
                        "p" => Control::Pause,
                        "r" => Control::Resume,
                        "q" => Control::Stop,
                        _ => {
                            tracing::info!("controls: p = pause new claims; r = resume; q = quit");
                            continue;
                        }
                    },
                    Err(error) => {
                        tracing::warn!(kind = ?error.kind(), "client input stopped");
                        break;
                    }
                };
                if send.blocking_send(control).is_err() {
                    break;
                }
            }
        })
        .context(StorageSnafu {
            operation: "start control input",
        })?;
    Ok(receive)
}

/// Runs the Windows client in the logged-in user's session, without a diagnostic mode.
///
/// # Errors
/// Stops claiming on invalid identity, damaged local state, storage failure or rejected authorization.
pub async fn run(config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let api = Api::new(&config)?;
    let mut store = Store::open(&config)?;
    let api = paired_api(&mut store, api).await?;
    let mut engine = Engine::new(store, api, NativePrinter)?;
    let mut input = controls()?;
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut stopping = false;
    let mut input_open = true;
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tracing::info!(
        paused = locked(&engine.store)?.state.paused,
        "print client ready; p = pause, r = resume, q = quit"
    );
    loop {
        if stopping && !engine.busy() {
            return Ok(());
        }
        tokio::select! {
            signal = &mut shutdown, if !stopping => {
                signal.context(StorageSnafu { operation: "listen for shutdown" })?;
                stopping = true;
            },
            command = input.recv(), if input_open => match command {
                Some(Control::Pause) => { locked(&engine.store)?.pause(true)?; tracing::info!("new claims paused; current work continues"); },
                Some(Control::Resume) => {
                    ensure!(!stopping, AttentionSnafu { reason: "client is shutting down" });
                    locked(&engine.store)?.pause(false)?;
                    tracing::info!("new claims enabled");
                },
                Some(Control::Stop) => stopping = true,
                None => input_open = false,
            },
            _ = tick.tick() => {
                if let Err(error) = engine.step(stopping).await {
                    match error {
                        ClientError::Transport { .. } | ClientError::Rejected { status: 429 | 500..=599 } | ClientError::Print { .. } => {
                            tracing::warn!(%error, "print operation will be retried without resubmitting the document");
                        },
                        error => {
                            if let Err(pause_error) = locked(&engine.store)?.pause(true) {
                                tracing::error!(%pause_error, "could not persist the stopped state");
                            }
                            return Err(error);
                        },
                    }
                }
            },
        }
    }
}
