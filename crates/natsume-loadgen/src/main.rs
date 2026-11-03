//! Synthetic workstation fleet and operator workloads over production HTTPS and WSS.

mod cli;
mod connection;
mod convergence;
mod emulate;
mod error;
mod identity;
mod logo;
mod metrics;
mod operator;
mod scenario;
mod setup;
mod target;

use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    process::ExitCode,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use clap::Parser as _;
use snafu::ResultExt as _;
use tokio::{task::JoinSet, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::{Cli, Command, Run},
    connection::Settings,
    emulate::Emulator,
    error::Result,
    identity::Identity,
    metrics::Metrics,
};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(false)
        .init();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let success = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            tracing::info!(message = %error, "command line");
            return if success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    let result = match cli.command {
        Command::Run(run) => execute(*run).await,
        Command::Setup(setup) => setup::execute(setup).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(code = error.code(), error = %snafu::Report::from_error(error), "loadgen failed");
            ExitCode::FAILURE
        }
    }
}

async fn execute(mut run: Run) -> Result<()> {
    if let Some(path) = &run.scenario {
        scenario::Scenario::load(path)?.apply(&mut run)?;
    }
    let seats = run.validate()?;
    let settings = Arc::new(Settings::new(&run)?);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&run.state_dir)
        .context(error::IoSnafu)?;
    let _lock = lock_fleet(&run)?;
    let run = Arc::new(run);
    let preparation = Arc::clone(&run);
    let identities = tokio::task::spawn_blocking(move || {
        seats
            .into_iter()
            .enumerate()
            .map(|(index, seat)| {
                Identity::load(&preparation.state_dir, preparation.seed, index, seat)
                    .and_then(|identity| Emulator::new(identity, preparation.seed, index))
            })
            .collect::<Result<Vec<_>>>()
    })
    .await
    .context(error::JoinSnafu)??;
    let (metrics, writer) = Metrics::start(&run.output_dir)?;
    write_manifest(&run, &metrics)?;
    let stop = CancellationToken::new();
    let started = Instant::now();
    let mut devices = JoinSet::new();
    operator::spawn(&mut devices, &run, &metrics, &stop, started)?;
    for (index, emulator) in identities.into_iter().enumerate() {
        let run = Arc::clone(&run);
        let settings = Arc::clone(&settings);
        let metrics = metrics.clone();
        let stop = stop.clone();
        devices.spawn(async move {
            tokio::select! { () = stop.cancelled() => return Ok(()), () = tokio::time::sleep_until(started + run.arrival_offset(index)) => {} }
            connection::run(emulator, index, run, settings, metrics, stop).await
        });
    }
    let result = supervise(&mut devices, &metrics, &run, &stop, started).await;
    stop.cancel();
    while let Some(result) = devices.join_next().await {
        if !matches!(result, Ok(Ok(()))) {
            tracing::error!("device failed during shutdown");
        }
    }
    tracing::info!(
        events = metrics.events.load(Ordering::Relaxed),
        connected = metrics.connected.load(Ordering::Relaxed),
        dropped_events = metrics.dropped.load(Ordering::Relaxed),
        convergence = %metrics.convergence.summary(),
        "fleet stopped"
    );
    write_manifest(&run, &metrics)?;
    drop(metrics);
    writer.await.context(error::JoinSnafu)??;
    result
}

fn lock_fleet(run: &Run) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(run.state_dir.join("fleet.lock"))
        .context(error::IoSnafu)?;
    file.try_lock().context(error::LockSnafu)?;
    Ok(file)
}

async fn supervise(
    devices: &mut JoinSet<Result<()>>,
    metrics: &Metrics,
    run: &Run,
    stop: &CancellationToken,
    started: Instant,
) -> Result<()> {
    let mut summary = tokio::time::interval(Duration::from_secs(10));
    summary.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut previous = 0;
    let mut sampled = started;
    loop {
        tokio::select! {
            () = tokio::time::sleep_until(started + run.ramp + run.duration) => { stop.cancel(); return Ok(()); }
            result = tokio::signal::ctrl_c() => { result.context(error::IoSnafu)?; stop.cancel(); return Ok(()); }
            _ = summary.tick() => {
                let events = metrics.events.load(Ordering::Relaxed);
                let elapsed = sampled.elapsed().as_secs_f64().max(0.001);
                tracing::info!(connected = metrics.connected.load(Ordering::Relaxed), devices = run.devices,
                    events_per_second = f64::from(u32::try_from(events - previous).unwrap_or(u32::MAX)) / elapsed, events, "fleet summary");
                previous = events;
                sampled = Instant::now();
            }
            result = devices.join_next() => match result {
                Some(result) => result.context(error::JoinSnafu)??,
                None => return Ok(()),
            }
        }
    }
}

fn write_manifest(run: &Run, metrics: &Metrics) -> Result<()> {
    let manifest = serde_json::json!({
        "format_version": 2, "version": env!("CARGO_PKG_VERSION"), "protocol": natsume_workstation_protocol::CONTROL_SUBPROTOCOL,
        "server": run.server, "devices": run.devices,
        "ramp_ms": run.ramp.as_millis(), "duration_ms": run.duration.as_millis(),
        "delay_ms": run.resource_delay.as_millis(), "jitter_ms": run.resource_jitter.as_millis(),
        "resource_delays_ms": ([run.gateway_delay, run.binding_delay, run.runtime_delay, run.session_delay, run.home_delay, run.power_delay].map(|d| d.map(|d| d.as_millis()))),
        "logo_interval_ms": run.logo_interval.as_millis(), "ping_interval_ms": run.ping_interval.as_millis(),
        "reconnect_initial_ms": run.reconnect_initial.as_millis(), "reconnect_max_ms": run.reconnect_max.as_millis(),
        "stable_after_ms": run.stable_after.as_millis(), "silence_timeout_ms": run.silence_timeout.as_millis(),
        "operators": run.operators, "poll_interval_ms": run.poll_interval.as_millis(), "session_interval_ms": run.session_interval.as_millis(),
        "schedule": run.events, "dropped_events": metrics.dropped.load(Ordering::Relaxed),
        "events": metrics.events.load(Ordering::Relaxed), "convergence": metrics.convergence.summary(),
    });
    fs::write(
        run.output_dir.join("run.json"),
        serde_json::to_vec_pretty(&manifest).context(error::JsonSnafu)?,
    )
    .context(error::IoSnafu)
}
