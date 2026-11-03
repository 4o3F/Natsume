use std::{collections::BTreeMap, sync::Mutex};

use natsume_workstation_protocol::generated::{
    ClientStateSnapshot, ForegroundTarget, HomeState, ServerStateSnapshot, SessionForeground,
    SessionState,
};
use serde_json::Value;
use tokio::time::Instant;

use crate::{
    error::{Error, Result},
    metrics::Metrics,
    scenario::Action,
};

#[derive(Default)]
pub(crate) struct Tracker(Mutex<Vec<Submission>>);
struct Submission {
    id: String,
    action: Action,
    response: Option<Instant>,
    devices: BTreeMap<String, Observation>,
    superseded: bool,
}
struct Observation {
    expected: Expected,
    index: Option<usize>,
    target: Option<Instant>,
    actual: Option<Instant>,
    target_emitted: bool,
    actual_emitted: bool,
}
#[derive(Clone, Copy)]
enum Expected {
    Foreground(bool),
    Terminate(u64),
    Home(u64),
}

impl Tracker {
    /// Arm before POST so observations arriving before its HTTP response are retained.
    pub(crate) fn begin(&self, id: String, action: Action, baseline: &[Value]) -> Result<()> {
        let mut devices = BTreeMap::new();
        for row in baseline.iter().filter(|row| row["state"] == "enabled") {
            let device = row["device_id"].as_str().ok_or(Error::Invalid {
                code: "DEVICE_RESPONSE",
            })?;
            let session = &row["convergence"]["session_control"]["target"];
            let expected = match action {
                Action::Waiting | Action::Contest => {
                    let contest = action == Action::Contest;
                    if session["foreground_target"] == if contest { "contest" } else { "waiting" } {
                        continue;
                    }
                    Expected::Foreground(contest)
                }
                Action::Terminate => Expected::Terminate(
                    session["terminate_epoch"]
                        .as_u64()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or(Error::Invalid {
                            code: "EPOCH_EXHAUSTED",
                        })?,
                ),
                Action::ResetHome => Expected::Home(
                    row["convergence"]["home"]["target_reset_epoch"]
                        .as_u64()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or(Error::Invalid {
                            code: "EPOCH_EXHAUSTED",
                        })?,
                ),
                Action::Export | Action::Import => {
                    return Err(Error::Invalid {
                        code: "CONVERGENCE_ACTION",
                    });
                }
            };
            devices.insert(
                device.to_owned(),
                Observation {
                    expected,
                    index: None,
                    target: None,
                    actual: None,
                    target_emitted: false,
                    actual_emitted: false,
                },
            );
        }
        let mut submissions = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("convergence tracker lock poisoned: {error}"));
        for previous in submissions.iter_mut() {
            if (previous.action == Action::ResetHome) == (action == Action::ResetHome) {
                previous.superseded = true;
            }
        }
        submissions.push(Submission {
            id,
            action,
            response: None,
            devices,
            superseded: false,
        });
        Ok(())
    }

    pub(crate) fn finish(
        &self,
        id: &str,
        response: Instant,
        receipt: &Value,
        metrics: &Metrics,
    ) -> Result<()> {
        let mut submissions = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("convergence tracker lock poisoned: {error}"));
        let submission = submissions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(Error::Invalid {
                code: "CONVERGENCE_SUBMISSION",
            })?;
        let results = receipt["results"].as_array().ok_or(Error::Invalid {
            code: "SUBMISSION_RECEIPT",
        })?;
        submission.devices.retain(|device, _| {
            results
                .iter()
                .any(|row| row["device_id"] == device.as_str() && row["status"] == "submitted")
        });
        submission.response = Some(response);
        for observed in submission.devices.values_mut() {
            emit(
                observed,
                submission.response,
                &submission.id,
                submission.action,
                metrics,
            )?;
        }
        Ok(())
    }

    pub(crate) fn target(
        &self,
        device: &str,
        index: usize,
        target: &ServerStateSnapshot,
        metrics: &Metrics,
    ) -> Result<()> {
        let Some(target) = &target.target else {
            return Ok(());
        };
        let mut submissions = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("convergence tracker lock poisoned: {error}"));
        for submission in submissions.iter_mut() {
            if submission.superseded {
                continue;
            }
            if let Some(row) = submission.devices.get_mut(device) {
                let matches = match row.expected {
                    Expected::Foreground(contest) => {
                        target.session_control.as_ref().is_some_and(|s| {
                            s.foreground_target
                                == i32::from(if contest {
                                    ForegroundTarget::Contest
                                } else {
                                    ForegroundTarget::Waiting
                                })
                        })
                    }
                    Expected::Terminate(epoch) => target
                        .session_control
                        .as_ref()
                        .is_some_and(|s| s.terminate_epoch == Some(epoch)),
                    Expected::Home(epoch) => target
                        .home
                        .as_ref()
                        .is_some_and(|h| h.reset_epoch == Some(epoch)),
                };
                if matches && row.target.is_none() {
                    row.target = Some(Instant::now());
                    row.index = Some(index);
                }
                emit(
                    row,
                    submission.response,
                    &submission.id,
                    submission.action,
                    metrics,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn actual(
        &self,
        device: &str,
        snapshot: &ClientStateSnapshot,
        metrics: &Metrics,
    ) -> Result<()> {
        let Some(actual) = &snapshot.actual else {
            return Ok(());
        };
        let mut submissions = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("convergence tracker lock poisoned: {error}"));
        for submission in submissions.iter_mut() {
            if submission.superseded {
                continue;
            }
            if let Some(row) = submission.devices.get_mut(device) {
                let matches = match row.expected {
                    Expected::Foreground(contest) => {
                        actual.session_control.as_ref().is_some_and(|s| {
                            s.session_state == i32::from(SessionState::Running)
                                && s.foreground
                                    == i32::from(if contest {
                                        SessionForeground::Contest
                                    } else {
                                        SessionForeground::Waiting
                                    })
                                && s.contest_ready
                                && s.waiting_ready
                        })
                    }
                    Expected::Terminate(epoch) => {
                        actual.session_control.as_ref().is_some_and(|s| {
                            s.completed_terminate_epoch == Some(epoch)
                                && s.session_state == i32::from(SessionState::Running)
                                && s.contest_ready
                                && s.waiting_ready
                        })
                    }
                    Expected::Home(epoch) => actual.home.as_ref().is_some_and(|h| {
                        h.completed_reset_epoch == Some(epoch)
                            && h.state == i32::from(HomeState::Steady)
                    }),
                };
                if matches && row.target.is_some() && row.actual.is_none() {
                    row.actual = Some(Instant::now());
                }
                emit(
                    row,
                    submission.response,
                    &submission.id,
                    submission.action,
                    metrics,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn summary(&self) -> Value {
        let submissions = self
            .0
            .lock()
            .unwrap_or_else(|error| panic!("convergence tracker lock poisoned: {error}"));
        Value::Array(submissions.iter().map(|s| serde_json::json!({
            "operation_id": s.id, "action": s.action, "expected": s.devices.len(),
            "target_received": s.devices.values().filter(|d| d.target.is_some()).count(),
            "actual_sent": s.devices.values().filter(|d| d.actual.is_some()).count(),
            "superseded": s.superseded,
        })).collect())
    }
}

fn emit(
    row: &mut Observation,
    response: Option<Instant>,
    id: &str,
    action: Action,
    metrics: &Metrics,
) -> Result<()> {
    let (Some(response), Some(index)) = (response, row.index) else {
        return Ok(());
    };
    for (time, emitted, kind) in [
        (row.target, &mut row.target_emitted, "convergence_target"),
        (row.actual, &mut row.actual_emitted, "convergence_actual"),
    ] {
        if let Some(time) = time
            && !*emitted
        {
            metrics.record_us(
                index,
                kind,
                delta_us(time, response),
                format!("{id}:{}", action.name()),
            )?;
            *emitted = true;
        }
    }
    Ok(())
}
fn delta_us(observation: Instant, response: Instant) -> i128 {
    if observation >= response {
        i128::try_from(observation.duration_since(response).as_micros()).unwrap_or(i128::MAX)
    } else {
        -i128::try_from(response.duration_since(observation).as_micros()).unwrap_or(i128::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::ResultExt as _;
    use std::io::Read as _;
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn early_target_and_actual_are_emitted_once_after_the_submission_response() -> Result<()>
    {
        use natsume_workstation_protocol::generated::{
            ActualState, ConcreteTargetState, SessionControlActualState, SessionControlTarget,
        };
        let directory = tempfile::tempdir().context(crate::error::IoSnafu)?;
        let output = directory.path().join("events");
        let (metrics, writer) = Metrics::start(&output)?;
        let tracker = Arc::clone(&metrics.convergence);
        let baseline = serde_json::json!([{"device_id":"device", "state":"enabled", "convergence":{"session_control":{"target":{"foreground_target":"waiting"}}}}]);
        let baseline = baseline.as_array().ok_or(Error::Invalid {
            code: "TEST_BASELINE",
        })?;
        tracker.begin("operation".to_owned(), Action::Contest, baseline)?;
        let target = ServerStateSnapshot {
            target: Some(ConcreteTargetState {
                session_control: Some(SessionControlTarget {
                    foreground_target: ForegroundTarget::Contest.into(),
                    terminate_epoch: None,
                }),
                ..ConcreteTargetState::default()
            }),
            ..ServerStateSnapshot::default()
        };
        tracker.target("device", 3, &target, &metrics)?;
        tokio::time::advance(std::time::Duration::from_millis(20)).await;
        let actual = ClientStateSnapshot {
            actual: Some(ActualState {
                session_control: Some(SessionControlActualState {
                    session_state: SessionState::Running.into(),
                    foreground: SessionForeground::Contest.into(),
                    waiting_ready: true,
                    contest_ready: true,
                    completed_terminate_epoch: None,
                }),
                ..ActualState::default()
            }),
            ..ClientStateSnapshot::default()
        };
        tracker.actual("device", &actual, &metrics)?;
        tokio::time::advance(std::time::Duration::from_millis(10)).await;
        tracker.finish(
            "operation",
            Instant::now(),
            &serde_json::json!({"results":[{"device_id":"device","status":"submitted"}]}),
            &metrics,
        )?;
        tracker.target("device", 3, &target, &metrics)?;
        tracker.actual("device", &actual, &metrics)?;
        assert_eq!(tracker.summary()[0]["actual_sent"], 1);
        drop(metrics);
        writer.await.context(crate::error::JoinSnafu)??;
        let file =
            std::fs::File::open(output.join("events.csv.gz")).context(crate::error::IoSnafu)?;
        let mut csv = String::new();
        flate2::read::GzDecoder::new(file)
            .read_to_string(&mut csv)
            .context(crate::error::IoSnafu)?;
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.contains(",3,convergence_target,-30000,operation:CONTEST"));
        assert!(csv.contains(",3,convergence_actual,-10000,operation:CONTEST"));
        Ok(())
    }
    #[test]
    fn response_race_preserves_negative_latencies() {
        let now = Instant::now();
        let later = now + std::time::Duration::from_millis(2);
        assert_eq!(delta_us(now, later), -2000);
        assert_eq!(delta_us(later, now), 2000);
    }
}
