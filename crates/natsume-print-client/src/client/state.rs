use std::{
    fs::{File, OpenOptions},
    io::{BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, ensure};

use super::{
    config::Config,
    error::{AttentionSnafu, RecordSnafu, Result, StorageSnafu},
    wire::{Attempt, Binding, Job, Report, canonical_id},
};

const RECORD_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Work {
    Idle,
    Claiming { request_id: String },
    Preparing { attempt: Attempt },
    Starting { attempt: Attempt },
    Observing { attempt: Attempt, job: Job },
    Reporting { attempt: Attempt, report: Report },
}

impl Work {
    pub fn attempt(&self) -> Option<&Attempt> {
        match self {
            Self::Idle | Self::Claiming { .. } => None,
            Self::Preparing { attempt }
            | Self::Starting { attempt }
            | Self::Observing { attempt, .. }
            | Self::Reporting { attempt, .. } => Some(attempt),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub server_url: String,
    pub queue_name: String,
    pub paused: bool,
    pub binding: Option<Binding>,
    pub work: Work,
}

impl State {
    fn validate(&self) -> Result<()> {
        if let Some(binding) = &self.binding {
            ensure!(
                canonical_id(&binding.printer_id) && canonical_id(&binding.client_id),
                AttentionSnafu {
                    reason: "invalid binding record"
                }
            );
            if !binding.confirmed {
                ensure!(
                    self.work == Work::Idle,
                    AttentionSnafu {
                        reason: "unfinished pairing has active work"
                    }
                );
            }
        } else {
            ensure!(
                self.work == Work::Idle,
                AttentionSnafu {
                    reason: "active work has no binding"
                }
            );
        }
        if let Work::Claiming { request_id } = &self.work {
            ensure!(
                canonical_id(request_id),
                AttentionSnafu {
                    reason: "invalid claim identity"
                }
            );
        }
        if let Some(attempt) = self.work.attempt() {
            let binding = self.binding.as_ref().ok_or_else(|| {
                AttentionSnafu {
                    reason: "missing binding",
                }
                .build()
            })?;
            attempt.validate(binding, &self.queue_name)?;
            ensure!(
                attempt.active(),
                AttentionSnafu {
                    reason: "local work contains a terminal attempt"
                }
            );
            match &self.work {
                Work::Observing { job, .. } => ensure!(
                    *job == attempt.job(job.job_id) && job.job_id > 0,
                    AttentionSnafu {
                        reason: "invalid local job identity"
                    }
                ),
                Work::Reporting { report, .. } => match report {
                    Report::Printed(job) => ensure!(
                        *job == attempt.job(job.job_id) && job.job_id > 0,
                        AttentionSnafu {
                            reason: "invalid completion identity"
                        }
                    ),
                    Report::Failed | Report::Unknown => {}
                    Report::Contact | Report::Observed(_) => {
                        return AttentionSnafu {
                            reason: "invalid saved result",
                        }
                        .fail();
                    }
                },
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    sequence: u64,
    state: State,
    sha256: String,
}

/// Appending and syncing avoids replacing an older, still-valid state file.
/// Any damaged tail stops recovery instead of silently replaying an older phase.
pub(super) struct Store {
    _lock: File,
    journal: File,
    sequence: u64,
    pub state: State,
}

fn lock(directory: &Path) -> Result<(File, bool)> {
    let path = directory.join("client.lock");
    let (lock, fresh) = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(lock) => (lock, true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&path).context(StorageSnafu {
                operation: "inspect client lock",
            })?;
            ensure!(
                metadata.is_file(),
                AttentionSnafu {
                    reason: "invalid client lock"
                }
            );
            (
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .context(StorageSnafu {
                        operation: "open client lock",
                    })?,
                false,
            )
        }
        Err(source) => {
            return Err(source).context(StorageSnafu {
                operation: "create client lock",
            });
        }
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return AttentionSnafu {
                reason: "another client owns this state directory",
            }
            .fail();
        }
        Err(std::fs::TryLockError::Error(source)) => {
            return Err(source).context(StorageSnafu {
                operation: "lock client state",
            });
        }
    }
    Ok((lock, fresh))
}

fn checksum(sequence: u64, state: &State) -> Result<String> {
    let bytes = serde_json::to_vec(&(sequence, state)).context(RecordSnafu)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn recover(file: &File) -> Result<(u64, Option<State>)> {
    let mut reader = BufReader::new(file);
    let mut sequence = 0_u64;
    let mut state = None;
    loop {
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(RECORD_LIMIT + 1)
            .read_until(b'\n', &mut bytes)
            .context(StorageSnafu {
                operation: "read state journal",
            })?;
        if bytes.is_empty() {
            break;
        }
        ensure!(
            bytes.len() as u64 <= RECORD_LIMIT && bytes.last() == Some(&b'\n'),
            AttentionSnafu {
                reason: "state journal has a damaged tail; do not roll back to an older phase"
            }
        );
        let envelope: Envelope = serde_json::from_slice(&bytes).context(RecordSnafu)?;
        ensure!(
            sequence.checked_add(1) == Some(envelope.sequence)
                && envelope.sha256 == checksum(envelope.sequence, &envelope.state)?,
            AttentionSnafu {
                reason: "state journal integrity check failed"
            }
        );
        envelope.state.validate()?;
        sequence = envelope.sequence;
        state = Some(envelope.state);
    }
    Ok((sequence, state))
}

impl Store {
    pub fn open(config: &Config) -> Result<Self> {
        std::fs::create_dir_all(&config.state_directory).context(StorageSnafu {
            operation: "create state directory",
        })?;
        let directory =
            std::fs::symlink_metadata(&config.state_directory).context(StorageSnafu {
                operation: "inspect state directory",
            })?;
        ensure!(
            directory.is_dir() && !directory.file_type().is_symlink(),
            AttentionSnafu {
                reason: "state directory is not a real directory"
            }
        );
        let (lock, fresh) = lock(&config.state_directory)?;
        let path = config.state_directory.join("state.jsonl");
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            ensure!(
                metadata.is_file(),
                AttentionSnafu {
                    reason: "state journal is not a regular file"
                }
            );
        }
        let journal = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(fresh)
            .open(&path)
            .context(StorageSnafu {
                operation: "open state journal; never reset an existing client",
            })?;
        let (sequence, recovered) = recover(&journal)?;
        ensure!(
            fresh || recovered.is_some(),
            AttentionSnafu {
                reason: "state journal is empty; do not reset an existing client"
            }
        );
        let state = recovered.unwrap_or_else(|| State {
            server_url: config.server_url.clone(),
            queue_name: config.queue_name.clone(),
            paused: true,
            binding: None,
            work: Work::Idle,
        });
        ensure!(
            state.server_url == config.server_url && state.queue_name == config.queue_name,
            AttentionSnafu {
                reason: "configured server or queue differs from the saved binding"
            }
        );
        let mut store = Self {
            _lock: lock,
            journal,
            sequence,
            state,
        };
        if fresh {
            store.save(store.state.clone())?;
            #[cfg(unix)]
            File::open(&config.state_directory)
                .and_then(|file| file.sync_all())
                .context(StorageSnafu {
                    operation: "sync new state directory",
                })?;
        }
        Ok(store)
    }

    pub fn save(&mut self, state: State) -> Result<()> {
        state.validate()?;
        let sequence = self.sequence.checked_add(1).ok_or_else(|| {
            AttentionSnafu {
                reason: "state journal sequence exhausted",
            }
            .build()
        })?;
        let sha256 = checksum(sequence, &state)?;
        let mut bytes = serde_json::to_vec(&Envelope {
            sequence,
            state: state.clone(),
            sha256,
        })
        .context(RecordSnafu)?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() as u64 <= RECORD_LIMIT,
            AttentionSnafu {
                reason: "state journal entry is too large"
            }
        );
        self.journal
            .seek(SeekFrom::End(0))
            .and_then(|_| self.journal.write_all(&bytes))
            .and_then(|()| self.journal.sync_all())
            .context(StorageSnafu {
                operation: "append and sync state journal",
            })?;
        self.sequence = sequence;
        self.state = state;
        Ok(())
    }

    pub fn work(&mut self, work: Work) -> Result<()> {
        let mut state = self.state.clone();
        state.work = work;
        self.save(state)
    }

    pub fn pause(&mut self, paused: bool) -> Result<()> {
        let mut state = self.state.clone();
        state.paused = paused;
        self.save(state)
    }
}

#[cfg(test)]
mod tests;
