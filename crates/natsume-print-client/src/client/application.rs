use std::path::Path;
use tokio::sync::{mpsc, watch};

use super::{
    api::Api,
    config::Config,
    engine::{Engine, Printer, locked},
    error::{ClientError, ControlClosedSnafu, Result},
    pairing::{self, Credentials},
    state::{State, Store, Work},
    wire::Report,
};

pub(super) enum Control {
    Pair(String),
    Pause,
    Resume,
    Stop,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Phase {
    Loading,
    #[default]
    Idle,
    Claiming,
    Preparing,
    Submitting,
    Observing,
    Reporting,
    Stopped,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum PairingStatus {
    #[default]
    Unknown,
    NeedsCode,
    Confirming,
    Confirmed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Problem {
    #[default]
    None,
    Configuration,
    Network,
    Printer,
    Attention,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Task {
    pub sequence: i64,
    pub attempt: i32,
    pub job: Option<u32>,
    pub label: String,
}

/// Presentation data only; credentials and document bytes never enter the UI channel.
#[derive(Clone, Debug, Default)]
pub(super) struct Snapshot {
    pub origin: String,
    pub queue: String,
    pub state_directory: String,
    pub ca_path: String,
    pub running: bool,
    pub pairing: PairingStatus,
    pub paused: bool,
    pub phase: Phase,
    pub task: Option<Task>,
    pub report_printed: bool,
    pub problem: Problem,
    pub message: String,
}

impl Snapshot {
    fn from_state(state: &State, config: &Config, message: &str) -> Self {
        let phase = match &state.work {
            Work::Idle => Phase::Idle,
            Work::Claiming { .. } => Phase::Claiming,
            Work::Preparing { .. } => Phase::Preparing,
            Work::Starting { .. } => Phase::Submitting,
            Work::Observing { .. } => Phase::Observing,
            Work::Reporting { .. } => Phase::Reporting,
        };
        let job = match &state.work {
            Work::Observing { job, .. }
            | Work::Reporting {
                report: Report::Printed(job),
                ..
            } => Some(job.job_id),
            _ => state.work.attempt().and_then(|attempt| attempt.job_id),
        };
        Self {
            origin: state.server_url.clone(),
            queue: state.queue_name.clone(),
            state_directory: config.state_directory.to_string_lossy().into_owned(),
            ca_path: config
                .ca_certificate
                .as_ref()
                .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
            running: true,
            paused: state.paused,
            pairing: if state
                .binding
                .as_ref()
                .is_some_and(|binding| binding.confirmed)
            {
                PairingStatus::Confirmed
            } else {
                PairingStatus::Unknown
            },
            phase,
            task: state.work.attempt().map(|attempt| Task {
                sequence: attempt.ready_sequence,
                attempt: attempt.attempt_number,
                job,
                label: attempt.label.clone(),
            }),
            report_printed: matches!(
                &state.work,
                Work::Reporting {
                    report: Report::Printed(_),
                    ..
                }
            ),
            message: message.to_owned(),
            ..Self::default()
        }
    }
}

pub(super) async fn run(
    path: &Path,
    credentials: &impl Credentials,
    printer: impl Printer,
    mut controls: mpsc::Receiver<Control>,
    snapshots: &watch::Sender<Snapshot>,
) -> Result<()> {
    let config = Config::load(path).inspect_err(|error| configuration_error(snapshots, error))?;
    snapshots.send_replace(Snapshot {
        origin: config.server_url.clone(),
        queue: config.queue_name.clone(),
        state_directory: config.state_directory.to_string_lossy().into_owned(),
        ca_path: config
            .ca_certificate
            .as_ref()
            .map_or_else(String::new, |path| path.to_string_lossy().into_owned()),
        running: true,
        paused: true,
        phase: Phase::Loading,
        ..Snapshot::default()
    });
    let mut api = Api::new(&config).inspect_err(|error| configuration_error(snapshots, error))?;
    let mut store = Store::open(&config)?;
    snapshots.send_replace(Snapshot::from_state(&store.state, &config, ""));
    if let Some(token) = pairing::restore(&mut store, credentials)? {
        api.authenticate(token);
    } else {
        snapshots.send_replace(Snapshot {
            pairing: PairingStatus::NeedsCode,
            ..Snapshot::from_state(&store.state, &config, "")
        });
        loop {
            match controls.recv().await {
                Some(Control::Pair(code)) => {
                    snapshots.send_replace(Snapshot {
                        pairing: PairingStatus::Confirming,
                        ..Snapshot::from_state(&store.state, &config, "")
                    });
                    api = pairing::complete(&mut store, api, credentials, code.trim()).await?;
                    break;
                }
                Some(Control::Stop) | None => return Ok(()),
                Some(Control::Pause | Control::Resume) => {}
            }
        }
    }
    let mut engine = Engine::new(store, api, printer)?;
    let mut stopping = false;
    let mut message = String::new();
    let mut problem = Problem::None;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(10));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let mut snapshot = Snapshot::from_state(&locked(&engine.store)?.state, &config, &message);
        if engine.busy() && snapshot.phase == Phase::Observing {
            snapshot.phase = Phase::Submitting;
        }
        snapshot.problem = problem;
        snapshots.send_replace(snapshot);
        if stopping && !engine.busy() {
            return Ok(());
        }
        tokio::select! {
            control = controls.recv(), if !stopping => match control {
                Some(Control::Pause) => locked(&engine.store)?.pause(true)?,
                Some(Control::Resume) => locked(&engine.store)?.pause(false)?,
                Some(Control::Stop) | None => { stopping = true; "Stopping new submissions; waiting for any native submission to return.".clone_into(&mut message); },
                Some(Control::Pair(_)) => {}
            },
            _ = tick.tick() => {
                match engine.step(stopping).await {
                    Ok(()) => { message.clear(); problem = Problem::None; },
                    Err(error @ (ClientError::Transport { .. } | ClientError::Rejected { status: 429 | 500..=599 } | ClientError::Print { .. })) => {
                        tracing::warn!(%error, "client work remains recorded; retrying without native resubmission");
                        problem = if matches!(error, ClientError::Print { .. }) { Problem::Printer } else { Problem::Network };
                        message = error.to_string();
                    },
                    Err(error) => {
                        if let Err(pause_error) = locked(&engine.store)?.pause(true) {
                            tracing::error!(%pause_error, "could not persist paused state");
                        }
                        return Err(error);
                    }
                }
            }
        }
    }
}

fn configuration_error(snapshots: &watch::Sender<Snapshot>, error: &ClientError) {
    let mut snapshot = snapshots.borrow().clone();
    snapshot.problem = Problem::Configuration;
    snapshot.message = error.to_string();
    snapshots.send_replace(snapshot);
}

pub(super) fn send(control: Control, sender: &mpsc::Sender<Control>) -> Result<()> {
    sender
        .try_send(control)
        .map_err(|_| ControlClosedSnafu.build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::ResultExt as _;

    #[test]
    fn idle_presentation_contains_no_credentials_and_preserves_pause() {
        let state = State {
            server_url: "https://example.test".to_owned(),
            queue_name: "Queue".to_owned(),
            paused: true,
            binding: None,
            work: Work::Idle,
        };
        let config = Config {
            server_url: state.server_url.clone(),
            queue_name: state.queue_name.clone(),
            state_directory: std::env::temp_dir().join("state"),
            ca_certificate: None,
        };
        let snapshot = Snapshot::from_state(&state, &config, "safe error");
        assert!(snapshot.paused);
        assert_eq!(snapshot.phase, Phase::Idle);
        assert!(snapshot.task.is_none());
        assert_eq!(snapshot.message, "safe error");
    }

    struct NoPrinter;
    impl Printer for NoPrinter {
        fn ready(&self, _: &str) -> crate::Result<bool> {
            crate::UnavailableSnafu.fail()
        }
        fn observe(&self, _: &str, _: u32, _: &str) -> crate::Result<crate::JobObservation> {
            crate::UnavailableSnafu.fail()
        }
        fn submit(
            &self,
            _: &str,
            _: &crate::Document,
            _: impl FnOnce(u32) -> crate::Result<()>,
        ) -> crate::Result<u32> {
            crate::UnavailableSnafu.fail()
        }
    }
    struct NoCredentials;
    impl Credentials for NoCredentials {
        fn load(&self, _: &str, _: &str) -> Result<String> {
            super::super::error::AttentionSnafu {
                reason: "unexpected credential read",
            }
            .fail()
        }
        fn save(&self, _: &str, _: &str, _: &str) -> Result<()> {
            super::super::error::AttentionSnafu {
                reason: "unexpected credential write",
            }
            .fail()
        }
        fn remove(&self, _: &str, _: &str) -> Result<()> {
            super::super::error::AttentionSnafu {
                reason: "unexpected credential removal",
            }
            .fail()
        }
    }

    #[tokio::test]
    async fn unpaired_application_waits_for_ui_and_can_stop_without_network_or_printing()
    -> Result<()> {
        let directory = tempfile::tempdir().context(super::super::error::StorageSnafu {
            operation: "create UI test directory",
        })?;
        let config = Config {
            server_url: "https://example.test".to_owned(),
            queue_name: "Queue".to_owned(),
            state_directory: directory.path().join("state"),
            ca_certificate: None,
        }
        .validated()?;
        let path = directory.path().join("client.toml");
        config.save_new(&path)?;
        assert!(config.save_new(&path).is_err());
        let (send, receive) = mpsc::channel(2);
        let (publish, mut snapshots) = watch::channel(Snapshot::default());
        let observer = async move {
            snapshots
                .changed()
                .await
                .map_err(|_| ControlClosedSnafu.build())?;
            let state = snapshots.borrow().clone();
            assert!(state.running && state.paused && state.pairing == PairingStatus::NeedsCode);
            assert_eq!(state.origin, "https://example.test");
            assert_eq!(state.queue, "Queue");
            send.send(Control::Stop)
                .await
                .map_err(|_| ControlClosedSnafu.build())
        };
        let (result, control_result) = tokio::join!(
            run(&path, &NoCredentials, NoPrinter, receive, &publish),
            observer
        );
        result?;
        control_result?;
        Ok(())
    }

    #[test]
    fn a_full_or_closed_control_channel_does_not_silently_drop_commands() -> Result<()> {
        let (sender, _receiver) = mpsc::channel(1);
        send(Control::Pause, &sender)?;
        assert!(send(Control::Resume, &sender).is_err());
        assert!(send(Control::Pair("TEST-CODE".to_owned()), &sender).is_err());
        Ok(())
    }
}
