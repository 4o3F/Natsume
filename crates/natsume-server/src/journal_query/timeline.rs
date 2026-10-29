//! Causal session reconstruction and retained device summaries.

use std::collections::{HashMap, VecDeque};

use serde::Serialize;
use uuid::Uuid;

use crate::{
    component::device::DeviceId,
    journal::{
        event::{
            Event,
            types::{Category, DisconnectReason, SeatCode, SessionId},
        },
        read::{Filter, JournalRecord, ReadRequest, RetainedBounds},
    },
};

use super::{
    JournalQuery, QueryError,
    dto::{DeviceSummaries, DeviceSummary, JournalHealth},
};

/// Observed terminal state, conservative end bound or verified live lease.
#[derive(Debug, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum SessionEnd {
    Disconnected {
        /// Observed terminal time, in Unix milliseconds.
        #[specta(type = specta_typescript::Number)]
        at: i64,
        /// Known closed reason; `None` for connection replacement.
        reason: Option<DisconnectReason>,
    },
    Unknown {
        /// Upper bound from a later boot/stop marker; `None` without evidence of an end time.
        #[specta(type = Option<specta_typescript::Number>)]
        no_later_than: Option<i64>,
    },
    Ongoing,
}

#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct TimelineSession {
    /// Lease identity from a typed attach or replacement event.
    pub(crate) session_id: Uuid,
    /// Writer boot in which the lease was attached.
    pub(crate) boot_id: Uuid,
    /// Observed attach time, in Unix milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub(crate) started_at: i64,
    /// Observed terminal, conservative end bound, or verified live state.
    pub(crate) end: SessionEnd,
    /// Visible start clipped to the requested interval, in Unix milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub(crate) visible_from: i64,
    /// Visible end clipped to the requested interval, in Unix milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub(crate) visible_to: i64,
    /// Known visible duration in milliseconds; `None` for unknown ends, zero on backwards clock steps.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) duration_ms: Option<i64>,
    /// True when attachment preceded the requested interval.
    pub(crate) truncated_start: bool,
    /// True when the lease may extend past the requested interval.
    pub(crate) truncated_end: bool,
    /// True when wall-clock order cannot provide a precise duration.
    pub(crate) imprecise: bool,
    /// Last committed binding snapshot observed for this lease; never current business state.
    pub(crate) seat_code: Option<SeatCode>,
}

#[derive(Debug, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GapKind {
    Pruned,
    Dropped,
    Paused,
    Unreadable,
}

#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct TimelineGap {
    /// Closed explanation for missing evidence.
    pub(crate) kind: GapKind,
    /// Lower gap bound in Unix milliseconds, clipped to the window.
    #[specta(type = specta_typescript::Number)]
    pub(crate) from: i64,
    /// Upper gap bound in Unix milliseconds; `None` while a pause remains unresolved.
    #[specta(type = Option<specta_typescript::Number>)]
    pub(crate) to: Option<i64>,
}

#[derive(Debug, Serialize, specta::Type)]
pub(crate) struct DeviceTimeline {
    /// Device requested by the authenticated operator.
    pub(crate) device_id: DeviceId,
    /// Reconstructed retained sessions, in journal causal order.
    pub(crate) sessions: Vec<TimelineSession>,
    /// Explicit retention, loss, pause and unreadable evidence gaps.
    pub(crate) gaps: Vec<TimelineGap>,
    /// Writer/retention health accompanying reconstruction.
    pub(crate) journal: JournalHealth,
}

impl JournalQuery {
    /// Reconstructs windowed sessions from causal facts and a final live-lease sample.
    pub(crate) async fn timeline(
        &self,
        device_id: DeviceId,
        from: i64,
        to: i64,
    ) -> Result<DeviceTimeline, QueryError> {
        if from > to {
            return Err(QueryError::Invalid);
        }
        let bounds = self.reader.bounds().await?;
        let baseline = self
            .timeline_baseline(device_id, from, bounds.high_water)
            .await?;
        let mut reconstruction = Reconstruction::new(from, to, baseline.seat_code, &bounds);
        reconstruction.pause = baseline.paused_at;
        let mut request = ReadRequest {
            timeline_device: Some(device_id),
            after: baseline.first_id.map(|id| id.saturating_sub(1)),
            high_water: bounds.high_water,
            ..ReadRequest::default()
        };
        loop {
            let page = self.scan_page(&mut request).await?;
            let more = page.has_more;
            for record in page.records {
                reconstruction.apply(record);
            }
            if !more {
                break;
            }
        }
        reconstruction.to = reconstruction.to.min(super::now_ms());
        let live = self.live.session(device_id).await.map(SessionId::value);
        let result = reconstruction.finish(live);
        Ok(DeviceTimeline {
            device_id,
            sessions: result.sessions,
            gaps: result.gaps,
            journal: self.health(&bounds),
        })
    }

    async fn clock_regressed_before_anchor(
        &self,
        device_id: DeviceId,
        anchor_id: i64,
        high_water: Option<i64>,
    ) -> Result<bool, QueryError> {
        let mut request = ReadRequest {
            timeline_device: Some(device_id),
            before: Some(anchor_id.saturating_add(1)),
            high_water,
            ..ReadRequest::default()
        };
        let mut previous = None;
        loop {
            let page = self.scan_page(&mut request).await?;
            for record in page.records {
                let JournalRecord::Readable { header, .. } = record else {
                    return Ok(true);
                };
                if previous.is_some_and(|previous| header.occurred_at_ms < previous) {
                    return Ok(true);
                }
                previous = Some(header.occurred_at_ms);
            }
            if !page.has_more {
                return Ok(false);
            }
        }
    }

    async fn timeline_baseline(
        &self,
        device_id: DeviceId,
        from: i64,
        high_water: Option<i64>,
    ) -> Result<TimelineBaseline, QueryError> {
        let anchor = self
            .reader
            .page(ReadRequest {
                filter: Filter {
                    device_id: Some(device_id),
                    to: Some(from),
                    kinds: vec![
                        "lease_attached".into(),
                        "lease_replaced".into(),
                        "disconnected".into(),
                        "actor_stopped".into(),
                    ],
                    ..Filter::default()
                },
                limit: 1,
                high_water,
                ..ReadRequest::default()
            })
            .await?;
        let mut start = anchor.records.first().map(JournalRecord::id);
        if anchor.records.first().is_some_and(|record| matches!(record, JournalRecord::Readable { details, .. } if matches!(details.event, Event::Disconnected(_) | Event::ActorStopped(_)))) {
            let opening = self.reader.page(ReadRequest {
                filter: Filter { device_id: Some(device_id), kinds: vec!["lease_attached".into(), "lease_replaced".into()], ..Filter::default() },
                before: start, high_water, limit: 1, ..ReadRequest::default()
            }).await?;
            start = opening.records.first().map(JournalRecord::id).or(start);
        }
        if let Some(anchor_id) = anchor.records.first().map(JournalRecord::id)
            && self
                .clock_regressed_before_anchor(device_id, anchor_id, high_water)
                .await?
        {
            // Wall-clock predicates cannot prove earlier sessions fall outside this window.
            start = None;
        }
        let mut seat = None;
        let mut paused_at = None;
        if let Some(start) = start {
            let binding = self
                .reader
                .page(ReadRequest {
                    filter: Filter {
                        device_id: Some(device_id),
                        kinds: vec!["binding_bound".into(), "binding_unbound".into()],
                        ..Filter::default()
                    },
                    before: Some(start),
                    limit: 1,
                    high_water,
                    ..ReadRequest::default()
                })
                .await?;
            seat = binding.records.first().and_then(|record| match record {
                JournalRecord::Readable { details, .. } => match &details.event {
                    Event::BindingBound(event) => Some(event.seat_code.clone()),
                    _ => None,
                },
                JournalRecord::Unreadable { .. } => None,
            });
            let pause = self
                .reader
                .page(ReadRequest {
                    filter: Filter {
                        kinds: vec!["journal_paused".into(), "journal_resumed".into()],
                        ..Filter::default()
                    },
                    before: Some(start),
                    limit: 1,
                    high_water,
                    ..ReadRequest::default()
                })
                .await?;
            paused_at = pause.records.first().and_then(|record| match record {
                JournalRecord::Readable { header, details }
                    if matches!(details.event, Event::JournalPaused(_)) =>
                {
                    Some(header.occurred_at_ms)
                }
                _ => None,
            });
        }
        Ok(TimelineBaseline {
            first_id: start,
            seat_code: seat,
            paused_at,
        })
    }

    async fn latest_summary_fact(
        &self,
        device_id: DeviceId,
        kinds: &[&str],
        high_water: i64,
    ) -> Result<Option<SummaryFact>, QueryError> {
        let mut request = ReadRequest {
            filter: Filter {
                device_id: Some(device_id),
                kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
                ..Filter::default()
            },
            high_water: Some(high_water),
            limit: 1,
            ..ReadRequest::default()
        };
        loop {
            let page = self.reader.page(request.clone()).await?;
            request.before = page.records.last().map(JournalRecord::id);
            for record in page.records {
                if let Some(fact) = summary_fact(record) {
                    return Ok(Some(fact));
                }
            }
            if !page.has_more {
                return Ok(None);
            }
        }
    }

    /// Reads latest per-device connection facts and only window-bounded terminal rows.
    pub(crate) async fn summaries(&self, window_ms: i64) -> Result<DeviceSummaries, QueryError> {
        if window_ms <= 0 {
            return Err(QueryError::Invalid);
        }
        let now = super::now_ms();
        let bounds = self.reader.bounds().await?;
        let mut summaries = HashMap::new();
        if let Some(high_water) = bounds.high_water {
            let mut after = None;
            loop {
                let page = self.reader.summary_devices(after, high_water).await?;
                for device_id in page.devices {
                    let connected = self
                        .latest_summary_fact(device_id, CONNECT_KINDS, high_water)
                        .await?;
                    let terminal = self
                        .latest_summary_fact(device_id, TERMINAL_KINDS, high_water)
                        .await?;
                    summaries.insert(
                        device_id,
                        DeviceSummary {
                            device_id,
                            last_connected_at: connected.map(|fact| fact.at),
                            last_disconnected_at: terminal.as_ref().map(|fact| fact.at),
                            last_disconnect_reason: terminal.and_then(|fact| fact.reason),
                            disconnects: 0,
                        },
                    );
                }
                let Some(cursor) = page.next_cursor else {
                    break;
                };
                after = Some(cursor);
            }
            let mut request = ReadRequest {
                filter: Filter {
                    kinds: TERMINAL_KINDS
                        .iter()
                        .map(|kind| (*kind).to_owned())
                        .collect(),
                    from: Some(now.saturating_sub(window_ms)),
                    to: Some(now),
                    ..Filter::default()
                },
                high_water: Some(high_water),
                ..ReadRequest::default()
            };
            loop {
                let page = self.scan_page(&mut request).await?;
                for record in page.records {
                    if let Some(fact) = summary_fact(record)
                        && let Some(summary) = summaries.get_mut(&fact.device_id)
                    {
                        summary.disconnects += 1;
                    }
                }
                if !page.has_more {
                    break;
                }
            }
        }
        let mut devices = summaries.into_values().collect::<Vec<_>>();
        devices.sort_by_key(|summary| summary.device_id.as_text());
        Ok(DeviceSummaries {
            devices,
            journal: self.health(&bounds),
        })
    }
}

/// Closed opening kinds used for last-connect lookup; replacement opens the new lease.
const CONNECT_KINDS: &[&str] = &["lease_attached", "lease_replaced"];
/// Closed terminal kinds used for latest-disconnect lookup and window counts.
const TERMINAL_KINDS: &[&str] = &["lease_replaced", "disconnected", "actor_stopped"];

struct SummaryFact {
    /// Validated identity from the readable connection row.
    device_id: DeviceId,
    /// Occurrence time of this connection fact in Unix milliseconds.
    at: i64,
    /// Exact terminal reason; None for an opening or replacement.
    reason: Option<DisconnectReason>,
}

fn summary_fact(record: JournalRecord) -> Option<SummaryFact> {
    let JournalRecord::Readable { header, details } = record else {
        return None;
    };
    let reason = match details.event {
        Event::LeaseAttached(_) | Event::LeaseReplaced(_) => None,
        Event::Disconnected(event) => Some(event.reason),
        Event::ActorStopped(event) if event.session_id.is_some() => {
            Some(DisconnectReason::ActorFailed)
        }
        _ => return None,
    };
    Some(SummaryFact {
        device_id: header.device_id?,
        at: header.occurred_at_ms,
        reason,
    })
}

/// Retained opening, binding and pause facts needed before forward replay.
struct TimelineBaseline {
    /// First causal row to replay, including an opening before a terminal anchor when needed.
    first_id: Option<i64>,
    /// Most recent committed binding snapshot before that row.
    seat_code: Option<SeatCode>,
    /// Unresolved writer pause before that row, in Unix milliseconds.
    paused_at: Option<i64>,
}

struct Reconstructed {
    /// Window-clipped sessions in causal journal order.
    sessions: Vec<TimelineSession>,
    /// Window-intersecting intervals with incomplete evidence.
    gaps: Vec<TimelineGap>,
}

/// Maximum retained session intervals in one timeline response and replay buffer.
const MAX_TIMELINE_SESSIONS: usize = 500;
/// Maximum retained evidence gaps in one timeline response and replay buffer.
const MAX_TIMELINE_GAPS: usize = 500;

struct Reconstruction {
    /// Requested wall-clock lower bound, in Unix milliseconds.
    from: i64,
    /// Requested wall-clock upper bound, in Unix milliseconds.
    to: i64,
    /// Committed seat snapshot maintained only by binding events.
    seat: Option<SeatCode>,
    /// Most recently opened lease until a terminal or newer boot is observed.
    current: Option<TimelineSession>,
    /// Recent window-intersecting sessions; bounded while replaying, clipped when finishing.
    sessions: VecDeque<TimelineSession>,
    /// Explicit intervals where reconstruction lacks complete evidence.
    gaps: VecDeque<TimelineGap>,
    /// True after a replay cap omitted older history; marks the first retained interval truncated.
    history_truncated: bool,
    /// Start time of an unresolved writer pause; `None` while writable.
    pause: Option<i64>,
}

impl Reconstruction {
    fn new(from: i64, to: i64, seat: Option<SeatCode>, bounds: &RetainedBounds) -> Self {
        let mut gaps = VecDeque::new();
        if let Some(oldest) = bounds.oldest_at
            && bounds.oldest_id.is_some_and(|id| id > 1)
            && oldest > from
        {
            gaps.push_back(TimelineGap {
                kind: GapKind::Pruned,
                from,
                to: Some(oldest.min(to)),
            });
        }
        Self {
            from,
            to,
            seat,
            current: None,
            sessions: VecDeque::new(),
            gaps,
            history_truncated: false,
            pause: None,
        }
    }

    fn completed_outside_window(&self, session: &TimelineSession) -> bool {
        matches!(session.end, SessionEnd::Disconnected { at, .. } if session.started_at > self.to || at.max(session.started_at) < self.from)
    }

    fn retain_session(&mut self, session: TimelineSession) {
        if self.completed_outside_window(&session) {
            return;
        }
        if self.sessions.len() == MAX_TIMELINE_SESSIONS {
            self.sessions.pop_front();
            self.history_truncated = true;
        }
        self.sessions.push_back(session);
    }

    fn retain_gap(&mut self, mut gap: TimelineGap) {
        if gap.from > self.to || gap.to.is_some_and(|at| at < self.from) {
            return;
        }
        gap.from = gap.from.max(self.from);
        gap.to = gap.to.map(|at| at.min(self.to));
        if self.gaps.len() == MAX_TIMELINE_GAPS {
            self.gaps.pop_front();
            self.history_truncated = true;
        }
        self.gaps.push_back(gap);
    }

    fn close(&mut self, session_id: Option<Uuid>, end: SessionEnd) {
        if self
            .current
            .as_ref()
            .is_some_and(|current| session_id.is_none_or(|id| id == current.session_id))
            && let Some(mut current) = self.current.take()
        {
            current.end = end;
            self.retain_session(current);
        } else if let Some(session_id) = session_id
            && let Some(index) = self
                .sessions
                .iter()
                .rposition(|session| session.session_id == session_id)
            && matches!(self.sessions[index].end, SessionEnd::Unknown { .. })
        {
            self.sessions[index].end = end;
            if self.completed_outside_window(&self.sessions[index]) {
                self.sessions.remove(index);
            }
        }
    }

    fn open(&mut self, session_id: Uuid, boot_id: Uuid, at: i64) {
        self.close(
            None,
            SessionEnd::Unknown {
                no_later_than: Some(at),
            },
        );
        self.current = Some(TimelineSession {
            session_id,
            boot_id,
            started_at: at,
            end: SessionEnd::Unknown {
                no_later_than: None,
            },
            visible_from: at,
            visible_to: self.to,
            duration_ms: None,
            truncated_start: false,
            truncated_end: false,
            imprecise: false,
            seat_code: self.seat.clone(),
        });
    }

    fn apply(&mut self, record: JournalRecord) {
        let JournalRecord::Readable { header, details } = record else {
            self.retain_gap(TimelineGap {
                kind: GapKind::Unreadable,
                from: self.from,
                to: Some(self.to),
            });
            return;
        };
        let at = header.occurred_at_ms;
        match details.event {
            Event::LeaseAttached(event) => self.open(event.session_id.value(), header.boot_id, at),
            Event::LeaseReplaced(event) => {
                self.close(
                    Some(event.old.value()),
                    SessionEnd::Disconnected { at, reason: None },
                );
                self.open(event.new.value(), header.boot_id, at);
            }
            Event::Disconnected(event) => self.close(
                Some(event.session_id.value()),
                SessionEnd::Disconnected {
                    at,
                    reason: Some(event.reason),
                },
            ),
            Event::ActorStopped(event) => {
                if let Some(id) = event.session_id {
                    self.close(
                        Some(id.value()),
                        SessionEnd::Disconnected {
                            at,
                            reason: Some(DisconnectReason::ActorFailed),
                        },
                    );
                }
            }
            Event::ServerStarted(event) => {
                if self
                    .current
                    .as_ref()
                    .is_some_and(|session| session.boot_id != event.boot_id)
                {
                    self.close(
                        None,
                        SessionEnd::Unknown {
                            no_later_than: Some(at),
                        },
                    );
                }
            }
            Event::ServerStopping(_) => self.close(
                None,
                SessionEnd::Unknown {
                    no_later_than: Some(at),
                },
            ),
            Event::BindingBound(event) => {
                self.seat = Some(event.seat_code);
                if let Some(current) = &mut self.current {
                    current.seat_code.clone_from(&self.seat);
                }
            }
            Event::BindingUnbound(_) => {
                self.seat = None;
                if let Some(current) = &mut self.current {
                    current.seat_code = None;
                }
            }
            Event::JournalDropped(event) => {
                if matches!(event.category, Category::Device | Category::Journal) {
                    self.retain_gap(TimelineGap {
                        kind: GapKind::Dropped,
                        from: event.since.max(self.from),
                        to: Some(at.min(self.to)),
                    });
                }
            }
            Event::JournalPaused(_) => self.pause = Some(at),
            Event::JournalResumed(_) => {
                if let Some(from) = self.pause.take() {
                    self.retain_gap(TimelineGap {
                        kind: GapKind::Paused,
                        from: from.max(self.from),
                        to: Some(at.min(self.to)),
                    });
                }
            }
            _ => {}
        }
    }

    fn finish(mut self, live: Option<Uuid>) -> Reconstructed {
        if let Some(mut current) = self.current.take() {
            if live == Some(current.session_id) {
                current.end = SessionEnd::Ongoing;
            }
            self.retain_session(current);
        }
        if let Some(from) = self.pause {
            self.retain_gap(TimelineGap {
                kind: GapKind::Paused,
                from: from.max(self.from),
                to: None,
            });
        }
        self.sessions.retain_mut(|session| {
            let end = match session.end {
                SessionEnd::Disconnected { at, .. } => Some(at),
                SessionEnd::Unknown { no_later_than } => no_later_than,
                SessionEnd::Ongoing => None,
            };
            let end_at = end.unwrap_or(self.to);
            let clock_back = end.is_some_and(|at| at < session.started_at);
            if session.started_at > self.to || end_at.max(session.started_at) < self.from {
                return false;
            }
            session.visible_from = session.started_at.max(self.from);
            session.visible_to = end_at.min(self.to).max(session.visible_from);
            session.duration_ms = if matches!(session.end, SessionEnd::Unknown { .. }) {
                None
            } else {
                Some(
                    session
                        .visible_to
                        .saturating_sub(session.visible_from)
                        .max(0),
                )
            };
            session.imprecise = clock_back;
            session.truncated_start = session.started_at < self.from;
            session.truncated_end = end.is_none_or(|at| at > self.to);
            true
        });
        self.gaps
            .retain(|gap| gap.from <= self.to && gap.to.is_none_or(|at| at >= self.from));
        if self.history_truncated
            && let Some(first) = self.sessions.front_mut()
        {
            first.truncated_start = true;
        }
        Reconstructed {
            sessions: self.sessions.into_iter().collect(),
            gaps: self.gaps.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests;
