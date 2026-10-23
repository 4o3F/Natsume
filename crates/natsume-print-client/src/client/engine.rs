use std::sync::{Arc, Mutex};

use snafu::{IntoError as _, ResultExt as _, ensure};
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::{
    api::Api,
    error::{AttentionSnafu, ClientError, PermissionSnafu, PrintSnafu, Result, WorkerSnafu},
    state::{Store, Work},
    wire::{Attempt, Binding, Job, Report},
};
use crate::{Document, JobObservation};

pub(super) trait Printer: Send + Sync + 'static {
    fn ready(&self, queue: &str) -> crate::Result<bool>;
    fn observe(&self, queue: &str, job: u32, label: &str) -> crate::Result<JobObservation>;
    fn submit(
        &self,
        queue: &str,
        document: &Document,
        started: impl FnOnce(u32) -> crate::Result<()>,
    ) -> crate::Result<u32>;
}

pub(super) struct Engine<P> {
    pub store: Arc<Mutex<Store>>,
    api: Arc<Api>,
    printer: Arc<P>,
    submitting: Option<JoinHandle<Result<u32>>>,
}

pub(super) fn locked(store: &Mutex<Store>) -> Result<std::sync::MutexGuard<'_, Store>> {
    store.lock().map_err(|_| {
        AttentionSnafu {
            reason: "local state lock was poisoned",
        }
        .build()
    })
}

async fn confirm_started(
    store: &Mutex<Store>,
    api: &Api,
    attempt: &Attempt,
    job_id: u32,
) -> Result<()> {
    let job = attempt.job(job_id);
    {
        let mut state = locked(store)?;
        ensure!(
            matches!(&state.state.work, Work::Starting { attempt: saved } if saved.same_identity(attempt)),
            AttentionSnafu {
                reason: "submission no longer owns the local attempt"
            }
        );
        state.work(Work::Observing {
            attempt: attempt.clone(),
            job: job.clone(),
        })?;
    }
    let reply = match api.register(&attempt.attempt_id, &job).await {
        Err(ClientError::Transport { .. }) => api.register(&attempt.attempt_id, &job).await,
        result => result,
    }?;
    let state = locked(store)?;
    let binding = state.state.binding.as_ref().ok_or_else(|| {
        AttentionSnafu {
            reason: "missing binding",
        }
        .build()
    })?;
    reply.validate(binding, &attempt.queue_name)?;
    ensure!(
        matches!(&state.state.work, Work::Observing { attempt: saved, job: saved_job }
            if saved.same_identity(attempt) && saved_job == &job),
        PermissionSnafu
    );
    ensure!(
        attempt.same_identity(&reply),
        AttentionSnafu {
            reason: "job registration changed the attempt identity"
        }
    );
    ensure!(
        reply.active() && reply.job_id == Some(job_id),
        PermissionSnafu
    );
    Ok(())
}

impl<P: Printer> Engine<P> {
    pub fn new(store: Store, api: Api, printer: P) -> Result<Self> {
        let binding = store.state.binding.as_ref().ok_or_else(|| {
            AttentionSnafu {
                reason: "client is not paired",
            }
            .build()
        })?;
        ensure!(
            binding.confirmed,
            AttentionSnafu {
                reason: "pairing was not completely confirmed"
            }
        );
        ensure!(
            !matches!(store.state.work, Work::Starting { .. }),
            AttentionSnafu {
                reason: "submission was interrupted without a saved Job ID; do not resubmit"
            }
        );
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            api: Arc::new(api),
            printer: Arc::new(printer),
            submitting: None,
        })
    }

    pub fn busy(&self) -> bool {
        self.submitting.is_some()
    }

    fn binding(&self) -> Result<Binding> {
        locked(&self.store)?.state.binding.clone().ok_or_else(|| {
            AttentionSnafu {
                reason: "missing binding",
            }
            .build()
        })
    }

    fn validate(&self, original: &Attempt, reply: &Attempt) -> Result<()> {
        reply.validate(&self.binding()?, &locked(&self.store)?.state.queue_name)?;
        ensure!(
            original.same_identity(reply)
                && original.job_id.is_none_or(|id| reply.job_id == Some(id)),
            AttentionSnafu {
                reason: "server changed the original attempt identity"
            }
        );
        Ok(())
    }

    fn save(&self, work: Work) -> Result<()> {
        locked(&self.store)?.work(work)
    }

    async fn finish_submission(&mut self) -> Result<()> {
        if self
            .submitting
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            let task = self.submitting.take().ok_or_else(|| {
                AttentionSnafu {
                    reason: "missing submission handle",
                }
                .build()
            })?;
            let result = task.await.map_err(|source| {
                tracing::error!(
                    panicked = source.is_panic(),
                    "print worker stopped unexpectedly"
                );
                WorkerSnafu.into_error(source)
            })?;
            if let Err(error) = result {
                tracing::error!(%error, "print submission did not finish normally");
                if matches!(
                    error,
                    ClientError::Storage { .. }
                        | ClientError::Record { .. }
                        | ClientError::Attention { .. }
                        | ClientError::Response
                ) {
                    return Err(error);
                }
                let work = locked(&self.store)?.state.work.clone();
                if let Some(attempt) = work.attempt() {
                    if matches!(work, Work::Starting { .. })
                        && matches!(
                            error,
                            ClientError::Print {
                                source: crate::PrintError::Unavailable
                            }
                        )
                    {
                        self.save(Work::Preparing {
                            attempt: attempt.clone(),
                        })?;
                    } else {
                        self.save(Work::Reporting {
                            attempt: attempt.clone(),
                            report: Report::Unknown,
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    /// One durable transition. Native submission runs separately so contacts and observations continue.
    pub async fn step(&mut self, stopping: bool) -> Result<()> {
        self.finish_submission().await?;
        let state = locked(&self.store)?.state.clone();
        match state.work {
            Work::Idle => {
                if state.paused || stopping || self.busy() {
                    return Ok(());
                }
                ensure!(
                    self.api.current().await?.is_none(),
                    AttentionSnafu {
                        reason: "server has an active attempt missing from local state"
                    }
                );
                let printer = Arc::clone(&self.printer);
                let queue = state.queue_name.clone();
                if !tokio::task::spawn_blocking(move || printer.ready(&queue))
                    .await
                    .context(WorkerSnafu)?
                    .context(PrintSnafu)?
                {
                    return Ok(());
                }
                let request_id = Uuid::now_v7().to_string();
                self.save(Work::Claiming {
                    request_id: request_id.clone(),
                })?;
                self.claim(&request_id, &state.queue_name).await
            }
            Work::Claiming { request_id } => self.claim(&request_id, &state.queue_name).await,
            Work::Preparing { attempt } => self.prepare(attempt, stopping).await,
            Work::Starting { attempt } => {
                ensure!(
                    self.busy(),
                    AttentionSnafu {
                        reason: "submission cannot be safely restarted"
                    }
                );
                let reply = self
                    .api
                    .report(&attempt.attempt_id, &Report::Contact)
                    .await?;
                self.validate(&attempt, &reply)?;
                if !reply.active() {
                    self.save(Work::Idle)?;
                }
                Ok(())
            }
            Work::Observing { attempt, job } => self.observe(attempt, job).await,
            Work::Reporting { attempt, report } => {
                let reply = self.api.report(&attempt.attempt_id, &report).await?;
                self.validate(&attempt, &reply)?;
                ensure!(
                    !reply.active(),
                    AttentionSnafu {
                        reason: "result was not confirmed"
                    }
                );
                tracing::info!(state = ?reply.state, "server confirmed the original attempt outcome; this is not proof of physical delivery");
                self.save(Work::Idle)
            }
        }
    }

    async fn observe(&self, attempt: Attempt, job: Job) -> Result<()> {
        let reply = self.api.attempt(&attempt.attempt_id).await?;
        self.validate(&attempt, &reply)?;
        if !reply.active() {
            return self.save(Work::Idle);
        }
        ensure!(
            reply.job_id.is_none_or(|id| id == job.job_id),
            AttentionSnafu {
                reason: "server Job ID does not match the saved job"
            }
        );
        let printer = Arc::clone(&self.printer);
        let observed_job = job.clone();
        let observation = tokio::task::spawn_blocking(move || {
            printer.observe(
                &observed_job.queue_name,
                observed_job.job_id,
                &observed_job.label,
            )
        })
        .await
        .context(WorkerSnafu)?
        .context(PrintSnafu)?;
        let report = match observation {
            JobObservation::Missing
            | JobObservation::Deleted
            | JobObservation::IdentityMismatch => Report::Unknown,
            JobObservation::SpoolerReportedPrinted if !self.busy() => Report::Printed(job.clone()),
            _ => Report::Observed(job.clone()),
        };
        if matches!(report, Report::Observed(_) | Report::Printed(_)) && reply.job_id.is_none() {
            let registered = self.api.register(&attempt.attempt_id, &job).await?;
            self.validate(&attempt, &registered)?;
            if !registered.active() {
                return self.save(Work::Idle);
            }
            ensure!(
                registered.job_id == Some(job.job_id),
                AttentionSnafu {
                    reason: "job registration was not confirmed"
                }
            );
        }
        if matches!(report, Report::Printed(_) | Report::Unknown) {
            self.save(Work::Reporting { attempt, report })
        } else {
            let reply = self.api.report(&attempt.attempt_id, &report).await?;
            self.validate(&attempt, &reply)?;
            if !reply.active() {
                self.save(Work::Idle)?;
            }
            if observation == JobObservation::NeedsAttention {
                tracing::warn!("printer job requires attention; no new work will be claimed");
            }
            Ok(())
        }
    }

    async fn claim(&self, request_id: &str, queue: &str) -> Result<()> {
        let Some(attempt) = self.api.claim(request_id, queue).await? else {
            return self.save(Work::Idle);
        };
        attempt.validate(&self.binding()?, queue)?;
        if !attempt.active() {
            return self.save(Work::Idle);
        }
        ensure!(
            attempt.job_id.is_none(),
            AttentionSnafu {
                reason: "claim reply contains a job missing from local state"
            }
        );
        self.save(Work::Preparing { attempt })
    }

    async fn prepare(&mut self, attempt: Attempt, stopping: bool) -> Result<()> {
        if stopping {
            return Ok(());
        }
        let reply = self
            .api
            .report(&attempt.attempt_id, &Report::Contact)
            .await?;
        self.validate(&attempt, &reply)?;
        if !reply.active() {
            return self.save(Work::Idle);
        }
        ensure!(
            reply.job_id.is_none(),
            AttentionSnafu {
                reason: "preparing attempt unexpectedly has a Windows job"
            }
        );
        let printer = Arc::clone(&self.printer);
        let queue = attempt.queue_name.clone();
        if !tokio::task::spawn_blocking(move || printer.ready(&queue))
            .await
            .context(WorkerSnafu)?
            .context(PrintSnafu)?
        {
            return Ok(());
        }
        let bytes = self.api.document(&attempt).await?;
        let document = Document::from_bytes(bytes, &attempt.attempt_id).context(PrintSnafu)?;
        self.save(Work::Starting {
            attempt: attempt.clone(),
        })?;
        let store = Arc::clone(&self.store);
        let api = Arc::clone(&self.api);
        let printer = Arc::clone(&self.printer);
        let runtime = tokio::runtime::Handle::current();
        self.submitting = Some(tokio::task::spawn_blocking(move || {
            let mut callback_error = None;
            let result = printer.submit(&attempt.queue_name, &document, |job_id| {
                if let Err(error) =
                    runtime.block_on(confirm_started(&store, &api, &attempt, job_id))
                {
                    callback_error = Some(error);
                    return crate::InvalidSnafu {
                        field: "server confirmation before first page",
                    }
                    .fail();
                }
                Ok(())
            });
            if let Some(error) = callback_error {
                return Err(error);
            }
            result.context(PrintSnafu)
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
