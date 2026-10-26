use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, JoinOnDsl as _, OptionalExtension as _,
    QueryDsl as _, RunQueryDsl as _, SelectableHelper as _,
    dsl::{exists, max},
};
use snafu::{OptionExt as _, ensure};
use uuid::Uuid;

use super::super::expiry::ActiveDeadline;
use super::super::{
    AuthenticationSnafu, ClientIdentity, ConflictSnafu, NotFoundSnafu, PrintError,
    execution::{
        AttemptOutcome, AttemptReport, AttemptState, ExecutionState, JobIdentity, PrintAttempt,
        PrintExecution, deadline,
    },
    hash_matches,
};
use crate::{
    db::Transaction,
    diesel_schema::{
        print_attempts as a, print_claims as c, print_documents as d, print_jobs as j,
        print_printers as p,
    },
};

pub(in crate::component::print) fn authenticate(
    tx: &mut Transaction<'_>,
    client: &ClientIdentity,
) -> Result<String, PrintError> {
    let (printer, hash, confirmed) = p::table
        .filter(p::client_id.eq(&client.client_id))
        .select((p::printer_id, p::client_token_hash, p::client_confirmed))
        .first::<(String, Option<Vec<u8>>, i32)>(tx.connection())
        .optional()?
        .context(AuthenticationSnafu)?;
    ensure!(
        confirmed == 1 && hash.is_some_and(|hash| hash_matches(&hash, &client.token_hash)),
        AuthenticationSnafu
    );
    Ok(printer)
}

#[derive(diesel::Queryable, diesel::Selectable)]
#[diesel(table_name = a)]
struct AttemptRow {
    attempt_id: String,
    ready_sequence: i64,
    printer_id: String,
    client_id: String,
    attempt_number: i32,
    queue_name: String,
    claimed_at_unix_ms: i64,
    prepare_expires_at_unix_ms: i64,
    lease_expires_at_unix_ms: i64,
    last_contact_at_unix_ms: i64,
    last_observed_at_unix_ms: Option<i64>,
    job_id: Option<i64>,
    finished_at_unix_ms: Option<i64>,
    outcome: Option<String>,
}

pub(in crate::component::print) fn attempt(
    tx: &mut Transaction<'_>,
    client: &ClientIdentity,
    id: &str,
) -> Result<PrintAttempt, PrintError> {
    authenticate(tx, client)?;
    let (row, (document_sha256, document_bytes, document_pages)) = a::table
        .inner_join(d::table.on(d::ready_sequence.eq(a::ready_sequence)))
        .filter(a::attempt_id.eq(id))
        .filter(a::client_id.eq(&client.client_id))
        .select((AttemptRow::as_select(), (d::sha256, d::bytes, d::pages)))
        .first::<(AttemptRow, (String, i64, i64))>(tx.connection())
        .optional()?
        .context(NotFoundSnafu)?;
    let state = match row
        .outcome
        .as_deref()
        .map(AttemptOutcome::parse)
        .transpose()?
    {
        Some(AttemptOutcome::Printed) => AttemptState::Printed,
        Some(AttemptOutcome::Failed) => AttemptState::Failed,
        Some(AttemptOutcome::Unknown) => AttemptState::Unknown,
        None if row.job_id.is_some() => AttemptState::Observing,
        None => AttemptState::Preparing,
    };
    Ok(PrintAttempt {
        label: format!("Natsume/{}", row.attempt_id),
        attempt_id: row.attempt_id,
        ready_sequence: row.ready_sequence,
        printer_id: row.printer_id,
        client_id: row.client_id,
        attempt_number: row.attempt_number,
        queue_name: row.queue_name,
        state,
        claimed_at_unix_ms: row.claimed_at_unix_ms,
        prepare_expires_at_unix_ms: row.prepare_expires_at_unix_ms,
        lease_expires_at_unix_ms: row.lease_expires_at_unix_ms,
        last_contact_at_unix_ms: row.last_contact_at_unix_ms,
        last_observed_at_unix_ms: row.last_observed_at_unix_ms,
        job_id: row.job_id,
        finished_at_unix_ms: row.finished_at_unix_ms,
        document_sha256,
        document_bytes,
        document_pages,
    })
}

pub(in crate::component::print) fn current(
    tx: &mut Transaction<'_>,
    client: &ClientIdentity,
) -> Result<Option<PrintAttempt>, PrintError> {
    authenticate(tx, client)?;
    let id = a::table
        .filter(a::client_id.eq(&client.client_id))
        .filter(a::outcome.is_null())
        .select(a::attempt_id)
        .first::<String>(tx.connection())
        .optional()?;
    id.map(|id| attempt(tx, client, &id)).transpose()
}

fn queue_order(tx: &mut Transaction<'_>) -> Result<i64, PrintError> {
    let last = j::table
        .select(max(j::queue_order))
        .first::<Option<i64>>(tx.connection())?
        .unwrap_or(0);
    deadline(last, 1)
}

pub(in crate::component::print) fn enqueue(
    tx: &mut Transaction<'_>,
    sequence: i64,
    now: i64,
) -> Result<(), PrintError> {
    let order = queue_order(tx)?;
    diesel::insert_into(j::table)
        .values((
            j::ready_sequence.eq(sequence),
            j::state.eq("queued"),
            j::attempts_used.eq(0),
            j::available_at_unix_ms.eq(now),
            j::queue_order.eq(order),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::print) fn claim(
    tx: &mut Transaction<'_>,
    client: &ClientIdentity,
    request: &str,
    queue: &str,
    now: i64,
) -> Result<Option<PrintAttempt>, PrintError> {
    let printer = authenticate(tx, client)?;
    expire(tx, now)?;
    if let Some((saved_queue, id)) = c::table
        .filter(c::client_id.eq(&client.client_id))
        .filter(c::request_id.eq(request))
        .select((c::queue_name, c::attempt_id))
        .first::<(String, Option<String>)>(tx.connection())
        .optional()?
    {
        ensure!(saved_queue == queue, ConflictSnafu);
        return id.map(|id| attempt(tx, client, &id)).transpose();
    }
    ensure!(current(tx, client)?.is_none(), ConflictSnafu);
    let next = j::table
        .filter(j::state.eq_any(["queued", "waiting"]))
        .filter(j::available_at_unix_ms.le(now))
        .order((j::available_at_unix_ms, j::queue_order))
        .select((j::ready_sequence, j::attempts_used, j::requeue_count))
        .first::<(i64, i32, i32)>(tx.connection())
        .optional()?;
    let id = if let Some((sequence, used, requeue_count)) = next {
        let id = Uuid::now_v7().to_string();
        diesel::insert_into(a::table)
            .values((
                a::attempt_id.eq(&id),
                a::ready_sequence.eq(sequence),
                a::printer_id.eq(printer),
                a::client_id.eq(&client.client_id),
                a::attempt_number.eq(used + 1),
                a::requeue_count.eq(requeue_count),
                a::queue_name.eq(queue),
                a::claimed_at_unix_ms.eq(now),
                a::prepare_expires_at_unix_ms.eq(deadline(now, 120_000)?),
                a::lease_expires_at_unix_ms.eq(deadline(now, 60_000)?),
                a::last_contact_at_unix_ms.eq(now),
            ))
            .execute(tx.connection())?;
        diesel::update(j::table.filter(j::ready_sequence.eq(sequence)))
            .set((j::state.eq("active"), j::attempts_used.eq(used + 1)))
            .execute(tx.connection())?;
        Some(id)
    } else {
        None
    };
    diesel::insert_into(c::table)
        .values((
            c::client_id.eq(&client.client_id),
            c::request_id.eq(request),
            c::queue_name.eq(queue),
            c::attempt_id.eq(id.as_deref()),
        ))
        .execute(tx.connection())?;
    id.map(|id| attempt(tx, client, &id)).transpose()
}

pub(in crate::component::print) fn requeue(
    tx: &mut Transaction<'_>,
    sequence: i64,
    expected_requeue_count: i32,
    now: i64,
) -> Result<(), PrintError> {
    let (state, requeue_count) = j::table
        .filter(j::ready_sequence.eq(sequence))
        .select((j::state, j::requeue_count))
        .first::<(String, i32)>(tx.connection())
        .optional()?
        .context(NotFoundSnafu)?;
    ensure!(
        state == "exhausted" && requeue_count == expected_requeue_count,
        ConflictSnafu
    );
    let next = requeue_count
        .checked_add(1)
        .context(super::super::PersistenceSnafu)?;
    let order = queue_order(tx)?;
    diesel::update(j::table.filter(j::ready_sequence.eq(sequence)))
        .set((
            j::state.eq("queued"),
            j::attempts_used.eq(0),
            j::requeue_count.eq(next),
            j::available_at_unix_ms.eq(now),
            j::queue_order.eq(order),
        ))
        .execute(tx.connection())?;
    Ok(())
}

fn finish(
    tx: &mut Transaction<'_>,
    id: &str,
    outcome: AttemptOutcome,
    now: i64,
) -> Result<(), PrintError> {
    let (sequence, number) = a::table
        .filter(a::attempt_id.eq(id))
        .filter(a::outcome.is_null())
        .select((a::ready_sequence, a::attempt_number))
        .first::<(i64, i32)>(tx.connection())?;
    diesel::update(a::table.filter(a::attempt_id.eq(id)))
        .set((
            a::outcome.eq(outcome.as_str()),
            a::finished_at_unix_ms.eq(now),
        ))
        .execute(tx.connection())?;
    let state = match outcome {
        AttemptOutcome::Printed => ExecutionState::Completed,
        _ if number >= 3 => ExecutionState::Exhausted,
        _ => ExecutionState::Waiting,
    };
    let available = if state == ExecutionState::Waiting {
        deadline(now, 30_000)?
    } else {
        now
    };
    let order = queue_order(tx)?;
    diesel::update(j::table.filter(j::ready_sequence.eq(sequence)))
        .set((
            j::state.eq(state.as_str()),
            j::available_at_unix_ms.eq(available),
            j::queue_order.eq(order),
            j::last_outcome.eq(outcome.as_str()),
        ))
        .execute(tx.connection())?;
    Ok(())
}

pub(in crate::component::print) fn expire(
    tx: &mut Transaction<'_>,
    now: i64,
) -> Result<(), PrintError> {
    let ids = a::table
        .filter(a::outcome.is_null())
        .filter(
            a::lease_expires_at_unix_ms.le(now).or(a::job_id
                .is_null()
                .and(a::prepare_expires_at_unix_ms.le(now))),
        )
        .order(a::attempt_id)
        .select(a::attempt_id)
        .load::<String>(tx.connection())?;
    for id in ids {
        finish(tx, &id, AttemptOutcome::Unknown, now)?;
    }
    Ok(())
}

type DeadlineRow = (String, i64, i64, Option<i64>);

fn active_deadline((attempt_id, prepare, lease, job_id): DeadlineRow) -> ActiveDeadline {
    ActiveDeadline {
        attempt_id,
        expires_at_unix_ms: if job_id.is_some() {
            lease
        } else {
            lease.min(prepare)
        },
    }
}

pub(in crate::component::print) fn load_active_deadlines(
    tx: &mut Transaction<'_>,
) -> Result<Vec<ActiveDeadline>, PrintError> {
    let rows = a::table
        .filter(a::outcome.is_null())
        .select((
            a::attempt_id,
            a::prepare_expires_at_unix_ms,
            a::lease_expires_at_unix_ms,
            a::job_id,
        ))
        .load::<DeadlineRow>(tx.connection())?;
    Ok(rows.into_iter().map(active_deadline).collect())
}

pub(in crate::component::print) fn check_attempts(
    tx: &mut Transaction<'_>,
    ids: &[String],
    now: i64,
) -> Result<Vec<ActiveDeadline>, PrintError> {
    let rows = a::table
        .filter(a::attempt_id.eq_any(ids))
        .filter(a::outcome.is_null())
        .select((
            a::attempt_id,
            a::prepare_expires_at_unix_ms,
            a::lease_expires_at_unix_ms,
            a::job_id,
        ))
        .load::<DeadlineRow>(tx.connection())?;
    let mut active = Vec::new();
    for row in rows {
        let deadline = active_deadline(row);
        if now >= deadline.expires_at_unix_ms {
            finish(tx, &deadline.attempt_id, AttemptOutcome::Unknown, now)?;
        } else {
            active.push(deadline);
        }
    }
    Ok(active)
}

pub(in crate::component::print) fn recover(
    tx: &mut Transaction<'_>,
    now: i64,
) -> Result<(), PrintError> {
    // Only registered, still-active jobs receive restart grace. Terminal attempts never revive.
    diesel::update(
        a::table
            .filter(a::outcome.is_null())
            .filter(a::job_id.is_not_null()),
    )
    .set(a::lease_expires_at_unix_ms.eq(deadline(now, 60_000)?))
    .execute(tx.connection())?;
    expire(tx, now)
}

fn matches_job(attempt: &PrintAttempt, job: &JobIdentity) -> bool {
    attempt.queue_name == job.queue_name
        && attempt.label == job.label
        && attempt.job_id == Some(job.job_id)
}

fn active(attempt: &PrintAttempt) -> bool {
    matches!(
        attempt.state,
        AttemptState::Preparing | AttemptState::Observing
    )
}

pub(in crate::component::print) fn register_job(
    tx: &mut Transaction<'_>,
    client: &ClientIdentity,
    id: &str,
    job: &JobIdentity,
    now: i64,
) -> Result<PrintAttempt, PrintError> {
    authenticate(tx, client)?;
    expire(tx, now)?;
    let current = attempt(tx, client, id)?;
    if !active(&current) {
        return Ok(current);
    }
    ensure!(
        current.queue_name == job.queue_name
            && current.label == job.label
            && (1..=i64::from(u32::MAX)).contains(&job.job_id),
        ConflictSnafu
    );
    if current.job_id.is_some() {
        ensure!(matches_job(&current, job), ConflictSnafu);
        // Replayed registration is not a spooler observation and does not renew the lease.
        return Ok(current);
    }
    diesel::update(a::table.filter(a::attempt_id.eq(id)))
        .set((
            a::job_id.eq(job.job_id),
            a::lease_expires_at_unix_ms.eq(deadline(now, 60_000)?),
            a::last_contact_at_unix_ms.eq(now),
        ))
        .execute(tx.connection())?;
    attempt(tx, client, id)
}

pub(in crate::component::print) fn report(
    tx: &mut Transaction<'_>,
    client: &ClientIdentity,
    id: &str,
    report: &AttemptReport,
    now: i64,
) -> Result<PrintAttempt, PrintError> {
    authenticate(tx, client)?;
    expire(tx, now)?;
    let current = attempt(tx, client, id)?;
    if !active(&current) {
        return Ok(current);
    }
    diesel::update(a::table.filter(a::attempt_id.eq(id)))
        .set(a::last_contact_at_unix_ms.eq(now))
        .execute(tx.connection())?;
    match report {
        AttemptReport::Contact if current.job_id.is_none() => {
            diesel::update(a::table.filter(a::attempt_id.eq(id)))
                .set(a::lease_expires_at_unix_ms.eq(deadline(now, 60_000)?))
                .execute(tx.connection())?;
        }
        AttemptReport::Contact => {}
        AttemptReport::Observed(job) => {
            ensure!(matches_job(&current, job), ConflictSnafu);
            diesel::update(a::table.filter(a::attempt_id.eq(id)))
                .set((
                    a::lease_expires_at_unix_ms.eq(deadline(now, 60_000)?),
                    a::last_observed_at_unix_ms.eq(now),
                ))
                .execute(tx.connection())?;
        }
        AttemptReport::Printed(job) => {
            ensure!(matches_job(&current, job), ConflictSnafu);
            diesel::update(a::table.filter(a::attempt_id.eq(id)))
                .set(a::last_observed_at_unix_ms.eq(now))
                .execute(tx.connection())?;
            finish(tx, id, AttemptOutcome::Printed, now)?;
        }
        AttemptReport::Failed => finish(tx, id, AttemptOutcome::Failed, now)?,
        AttemptReport::Unknown => finish(tx, id, AttemptOutcome::Unknown, now)?,
    }
    attempt(tx, client, id)
}

pub(in crate::component::print) fn has_active_printer(
    tx: &mut Transaction<'_>,
    printer: &str,
) -> Result<bool, PrintError> {
    Ok(diesel::select(exists(
        a::table
            .filter(a::printer_id.eq(printer))
            .filter(a::outcome.is_null()),
    ))
    .get_result(tx.connection())?)
}

pub(in crate::component::print) fn list(
    tx: &mut Transaction<'_>,
    now: i64,
) -> Result<Vec<PrintExecution>, PrintError> {
    let rows = j::table
        .order(j::ready_sequence)
        .select((
            j::ready_sequence,
            j::state,
            j::attempts_used,
            j::available_at_unix_ms,
            j::last_outcome,
            j::requeue_count,
        ))
        .load::<(i64, String, i32, i64, Option<String>, i32)>(tx.connection())?;
    rows.into_iter()
        .map(
            |(ready_sequence, state, attempts_used, available, outcome, requeue_count)| {
                let state = ExecutionState::parse(&state)?;
                Ok(PrintExecution {
                    ready_sequence,
                    state: if state == ExecutionState::Waiting && available <= now {
                        ExecutionState::Queued
                    } else {
                        state
                    },
                    attempts_used,
                    requeue_count,
                    retry_at_unix_ms: (state == ExecutionState::Waiting && available > now)
                        .then_some(available),
                    last_outcome: outcome.as_deref().map(AttemptOutcome::parse).transpose()?,
                })
            },
        )
        .collect()
}
