mod expiry;
mod requeue;

use super::{Fixture, pdf, registration};
use crate::component::print::{
    ClientIdentity, PrintError,
    db::execution as store,
    execution::{AttemptOutcome, AttemptReport, AttemptState, JobIdentity, PrintAttempt},
};
use crate::db::Transaction;
use uuid::Uuid;

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("print execution fixture failed: {error:?}"))
}

async fn write<T: Send + 'static>(
    f: &Fixture,
    operation: impl FnOnce(&mut Transaction<'_>) -> Result<T, PrintError> + Send + 'static,
) -> Result<T, PrintError> {
    f.database
        .write(operation)
        .await
        .map_err(crate::db::TransactionError::into_error)
}

async fn client(f: &Fixture) -> ClientIdentity {
    let printer = must(f.print.create_printer("Printer".to_owned()).await);
    let code = must(f.print.issue_pairing_code(printer.printer_id).await);
    let issued = must(f.print.pair_client(&code.code).await);
    must(f.print.confirm_client(&issued.token).await);
    must(f.print.authenticate_client(&issued.token).await)
}

async fn ready(f: &Fixture) -> i64 {
    let input = registration();
    let id = input.request_id.clone();
    must(f.print.register(f.source.clone(), input).await);
    let request = must(f.print.upload(f.source.clone(), id, pdf(1)).await);
    request
        .admission
        .unwrap_or_else(|| panic!("missing admission"))
        .ready_sequence
}

async fn claim(
    f: &Fixture,
    client: &ClientIdentity,
    request: &str,
    now: i64,
) -> Option<PrintAttempt> {
    let client = client.clone();
    let request = request.to_owned();
    must(
        write(f, move |tx| {
            store::claim(tx, &client, &request, "queue", now)
        })
        .await,
    )
}

fn job(attempt: &PrintAttempt) -> JobIdentity {
    JobIdentity {
        queue_name: "queue".to_owned(),
        job_id: 42,
        label: format!("Natsume/{}", attempt.attempt_id),
    }
}

async fn report(
    f: &Fixture,
    client: &ClientIdentity,
    attempt: &PrintAttempt,
    report: AttemptReport,
    now: i64,
) -> Result<PrintAttempt, PrintError> {
    let client = client.clone();
    let id = attempt.attempt_id.clone();
    write(f, move |tx| store::report(tx, &client, &id, &report, now)).await
}

#[tokio::test]
async fn claims_are_durable_idempotent_and_empty_results_do_not_later_claim() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let empty = Uuid::now_v7().to_string();
    assert!(claim(&f, &c, &empty, 10).await.is_none());
    let sequence = ready(&f).await;
    // Admission uses real database time; explicit test time starts after it.
    let now = must(write(&f, super::super::db::printers::now).await);
    assert!(claim(&f, &c, &empty, now).await.is_none());
    let request = Uuid::now_v7().to_string();
    let first = claim(&f, &c, &request, now)
        .await
        .unwrap_or_else(|| panic!("missing claim"));
    assert_eq!(first.ready_sequence, sequence);
    assert_eq!(first.attempt_number, 1);
    assert_eq!(claim(&f, &c, &request, now + 1).await, Some(first.clone()));
    let other = c.clone();
    assert_eq!(
        write(&f, move |tx| store::claim(
            tx,
            &other,
            &Uuid::now_v7().to_string(),
            "queue",
            now + 1
        ))
        .await,
        Err(PrintError::Conflict)
    );
    let other = c.clone();
    let reused = request.clone();
    assert_eq!(
        write(&f, move |tx| store::claim(
            tx,
            &other,
            &reused,
            "another-queue",
            now
        ))
        .await,
        Err(PrintError::Conflict)
    );
    let reopened = must(
        crate::db::Database::connect_and_migrate(&crate::db::DatabaseConfig::new(&f.path, false))
            .await,
    );
    let reloaded =
        must(crate::component::print::PrintComponent::load(reopened, f.documents_path()).await);
    let replay_empty = crate::component::print::execution::PrintClaim {
        request_id: empty,
        queue_name: "queue".to_owned(),
    };
    assert!(must(reloaded.claim(c.clone(), replay_empty).await).is_none());
    let saved = must(reloaded.current_attempt(c).await);
    assert_eq!(saved, Some(first));
}

#[tokio::test]
async fn concurrent_claims_have_one_owner_and_one_attempt() {
    let f = Fixture::new().await;
    let a = client(&f).await;
    let b = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let one_id = Uuid::now_v7().to_string();
    let two_id = Uuid::now_v7().to_string();
    let (one, two) = tokio::join!(claim(&f, &a, &one_id, now), claim(&f, &b, &two_id, now));
    assert_ne!(one.is_some(), two.is_some());
    let executions = must(f.print.list_executions().await);
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0].attempts_used, 1);
}

#[tokio::test]
async fn retry_waits_thirty_seconds_and_joins_tail_with_three_attempt_cap() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let sequence = ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let first = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let failed = must(report(&f, &c, &first, AttemptReport::Failed, now + 1).await);
    assert_eq!(failed.state, AttemptState::Failed);
    assert!(
        claim(&f, &c, &Uuid::now_v7().to_string(), now + 30_000)
            .await
            .is_none()
    );
    let later_sequence = ready(&f).await;
    let later = claim(&f, &c, &Uuid::now_v7().to_string(), now + 30_001)
        .await
        .unwrap_or_else(|| panic!("later claim"));
    assert_eq!(later.ready_sequence, later_sequence);
    must(report(&f, &c, &later, AttemptReport::Failed, now + 30_002).await);
    for (number, time) in [(2, now + 30_003), (3, now + 60_005)] {
        let attempt = claim(&f, &c, &Uuid::now_v7().to_string(), time)
            .await
            .unwrap_or_else(|| panic!("retry"));
        assert_eq!(attempt.ready_sequence, sequence);
        assert_eq!(attempt.attempt_number, number);
        must(report(&f, &c, &attempt, AttemptReport::Failed, time + 1).await);
        if number == 2 {
            // The other task became eligible earlier than this task's third attempt.
            let other = claim(&f, &c, &Uuid::now_v7().to_string(), now + 60_003)
                .await
                .unwrap_or_else(|| panic!("other retry"));
            assert_eq!(other.ready_sequence, later_sequence);
            must(report(&f, &c, &other, AttemptReport::Failed, now + 60_004).await);
        }
    }
    let executions = must(f.print.list_executions().await);
    let exhausted = executions
        .iter()
        .find(|j| j.ready_sequence == sequence)
        .unwrap_or_else(|| panic!("execution"));
    assert_eq!(exhausted.state.as_str(), "exhausted");
    assert_eq!(exhausted.attempts_used, 3);
    assert_eq!(exhausted.last_outcome, Some(AttemptOutcome::Failed));
}

#[tokio::test]
async fn prepare_deadline_and_contact_deadline_are_independent_and_exact() {
    for heartbeat in [false, true] {
        let f = Fixture::new().await;
        let c = client(&f).await;
        ready(&f).await;
        let now = must(write(&f, super::super::db::printers::now).await);
        let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
            .await
            .unwrap_or_else(|| panic!("claim"));
        if heartbeat {
            for offset in [50_000, 100_000, 119_999] {
                must(report(&f, &c, &a, AttemptReport::Contact, now + offset).await);
            }
        }
        let deadline = if heartbeat { 120_000 } else { 60_000 };
        must(write(&f, move |tx| store::expire(tx, now + deadline - 1)).await);
        let identity = c.clone();
        assert!(must(write(&f, move |tx| store::current(tx, &identity)).await).is_some());
        must(write(&f, move |tx| store::expire(tx, now + deadline)).await);
        let result = must(report(&f, &c, &a, AttemptReport::Contact, now + deadline).await);
        assert_eq!(result.state, AttemptState::Unknown);
    }
}

#[tokio::test]
async fn job_registration_is_exact_and_only_matching_observation_renews_afterward() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let identity = c.clone();
    let id = a.attempt_id.clone();
    let j = job(&a);
    let started = must(
        write(&f, move |tx| {
            store::register_job(tx, &identity, &id, &j, now + 1)
        })
        .await,
    );
    assert_eq!(started.state, AttemptState::Observing);
    for offset in [50_000, 100_000, 3_600_000] {
        // Simulate repeated exact observations without sleeping, including a long paper-out wait.
        let end = if offset == 3_600_000 {
            offset
        } else {
            offset + 1
        };
        let begin = if offset == 3_600_000 { 150_000 } else { offset };
        for elapsed in (begin..=end).step_by(40_000) {
            must(report(&f, &c, &a, AttemptReport::Observed(job(&a)), now + elapsed).await);
        }
    }
    let last = now + 3_590_000;
    let contacted = must(report(&f, &c, &a, AttemptReport::Contact, last + 1).await);
    let deadline = contacted.lease_expires_at_unix_ms;
    must(write(&f, move |tx| store::expire(tx, deadline)).await);
    assert_eq!(
        must(report(&f, &c, &a, AttemptReport::Observed(job(&a)), deadline).await).state,
        AttemptState::Unknown
    );
}

#[tokio::test]
async fn success_is_idempotent_and_late_reports_cannot_finish_a_new_attempt() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    assert_eq!(
        report(&f, &c, &a, AttemptReport::Printed(job(&a)), now).await,
        Err(PrintError::Conflict)
    );
    let identity = c.clone();
    let id = a.attempt_id.clone();
    let j = job(&a);
    must(
        write(&f, move |tx| {
            store::register_job(tx, &identity, &id, &j, now + 1)
        })
        .await,
    );
    let printed = must(report(&f, &c, &a, AttemptReport::Printed(job(&a)), now + 2).await);
    assert_eq!(printed.state, AttemptState::Printed);
    assert_eq!(
        must(report(&f, &c, &a, AttemptReport::Printed(job(&a)), now + 500_000).await),
        printed
    );
    assert!(
        claim(&f, &c, &Uuid::now_v7().to_string(), now + 500_001)
            .await
            .is_none()
    );
    ready(&f).await;
    let old = claim(&f, &c, &Uuid::now_v7().to_string(), now + 500_002)
        .await
        .unwrap_or_else(|| panic!("claim"));
    must(report(&f, &c, &old, AttemptReport::Unknown, now + 500_003).await);
    let new = claim(&f, &c, &Uuid::now_v7().to_string(), now + 530_003)
        .await
        .unwrap_or_else(|| panic!("retry"));
    let late = must(
        report(
            &f,
            &c,
            &old,
            AttemptReport::Printed(job(&old)),
            now + 530_004,
        )
        .await,
    );
    assert_eq!(late.state, AttemptState::Unknown);
    let identity = c.clone();
    assert_eq!(
        must(write(&f, move |tx| store::current(tx, &identity)).await),
        Some(new)
    );
}

#[tokio::test]
async fn due_retry_precedes_later_admission_even_without_intervening_poll() {
    use crate::diesel_schema::print_jobs;
    use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
    let f = Fixture::new().await;
    let c = client(&f).await;
    let first_sequence = ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    must(report(&f, &c, &a, AttemptReport::Failed, now + 1).await);
    let later_sequence = ready(&f).await;
    // Model a database-clock admission after the retry became eligible, without a sweep.
    must(
        write(&f, move |tx| {
            diesel::update(print_jobs::table.filter(print_jobs::ready_sequence.eq(later_sequence)))
                .set(print_jobs::available_at_unix_ms.eq(now + 40_000))
                .execute(tx.connection())?;
            Ok(())
        })
        .await,
    );
    let retry = claim(&f, &c, &Uuid::now_v7().to_string(), now + 50_000)
        .await
        .unwrap_or_else(|| panic!("retry"));
    assert_eq!(retry.ready_sequence, first_sequence);
}

#[tokio::test]
async fn binding_clear_and_stale_identity_are_guarded_by_persisted_active_attempt() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    assert_eq!(
        f.print.clear_printer_binding(a.printer_id.clone()).await,
        Err(PrintError::Conflict)
    );
    must(write(&f, move |tx| store::expire(tx, now + 60_000)).await);
    must(f.print.clear_printer_binding(a.printer_id.clone()).await);
    assert_eq!(
        f.print.get_attempt(c.clone(), a.attempt_id.clone()).await,
        Err(PrintError::Authentication)
    );
    assert_eq!(
        f.print.attempt_document(c, a.attempt_id).await,
        Err(PrintError::Authentication)
    );
}

#[tokio::test]
async fn concurrent_claim_and_binding_clear_cannot_leave_a_revoked_owner() {
    use crate::component::print::execution::PrintClaim;
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let identity = c.clone();
    let printer = must(write(&f, move |tx| store::authenticate(tx, &identity)).await);
    let request = PrintClaim {
        request_id: Uuid::now_v7().to_string(),
        queue_name: "queue".to_owned(),
    };
    let (claimed, cleared) = tokio::join!(
        f.print.claim(c, request),
        f.print.clear_printer_binding(printer)
    );
    match (claimed, cleared) {
        (Ok(Some(attempt)), Err(PrintError::Conflict)) => assert_eq!(attempt.attempt_number, 1),
        (Err(PrintError::Authentication), Ok(())) => {}
        other => panic!("claim and binding clear broke ownership: {other:?}"),
    }
}

#[tokio::test]
async fn registration_and_observation_mismatches_do_not_renew_or_replace_job() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let identity = c.clone();
    let id = a.attempt_id.clone();
    let j = job(&a);
    let registered = must(
        write(&f, move |tx| {
            store::register_job(tx, &identity, &id, &j, now + 1)
        })
        .await,
    );
    for mismatch in [
        JobIdentity {
            job_id: 43,
            ..job(&a)
        },
        JobIdentity {
            queue_name: "other".to_owned(),
            ..job(&a)
        },
        JobIdentity {
            label: "Natsume/other".to_owned(),
            ..job(&a)
        },
    ] {
        assert_eq!(
            report(
                &f,
                &c,
                &a,
                AttemptReport::Observed(mismatch.clone()),
                now + 50_000
            )
            .await,
            Err(PrintError::Conflict)
        );
        let identity = c.clone();
        let id = a.attempt_id.clone();
        assert_eq!(
            write(&f, move |tx| store::register_job(
                tx,
                &identity,
                &id,
                &mismatch,
                now + 50_000
            ))
            .await,
            Err(PrintError::Conflict)
        );
    }
    let identity = c.clone();
    let id = a.attempt_id.clone();
    let j = job(&a);
    let replay = must(
        write(&f, move |tx| {
            store::register_job(tx, &identity, &id, &j, now + 50_000)
        })
        .await,
    );
    assert_eq!(replay, registered);
    assert_eq!(
        must(report(&f, &c, &a, AttemptReport::Contact, now + 60_001).await).state,
        AttemptState::Unknown
    );
}

#[tokio::test]
async fn failed_claim_and_report_transactions_preserve_attempt_budget_and_receipts() {
    use diesel::connection::SimpleConnection as _;
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    must(write(&f, |tx| { tx.connection().batch_execute("CREATE TRIGGER reject_claim BEFORE INSERT ON print_claims BEGIN SELECT RAISE(ABORT, 'fixture'); END")?; Ok(()) }).await);
    let request = Uuid::now_v7().to_string();
    let identity = c.clone();
    let request_copy = request.clone();
    assert_eq!(
        write(&f, move |tx| store::claim(
            tx,
            &identity,
            &request_copy,
            "queue",
            now
        ))
        .await,
        Err(PrintError::Persistence)
    );
    assert_eq!(must(f.print.list_executions().await)[0].attempts_used, 0);
    must(
        write(&f, |tx| {
            tx.connection().batch_execute("DROP TRIGGER reject_claim")?;
            Ok(())
        })
        .await,
    );
    let a = claim(&f, &c, &request, now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    assert_eq!(a.attempt_number, 1);
    must(write(&f, |tx| { tx.connection().batch_execute("CREATE TRIGGER reject_job_update BEFORE UPDATE ON print_jobs BEGIN SELECT RAISE(ABORT, 'fixture'); END")?; Ok(()) }).await);
    assert_eq!(
        report(&f, &c, &a, AttemptReport::Failed, now + 1).await,
        Err(PrintError::Persistence)
    );
    let identity = c.clone();
    assert_eq!(
        must(write(&f, move |tx| store::current(tx, &identity)).await),
        Some(a.clone())
    );
    must(
        write(&f, |tx| {
            tx.connection()
                .batch_execute("DROP TRIGGER reject_job_update")?;
            Ok(())
        })
        .await,
    );
    assert_eq!(
        must(report(&f, &c, &a, AttemptReport::Failed, now + 2).await).state,
        AttemptState::Failed
    );
}

#[tokio::test]
async fn maintenance_expires_attempts_without_any_client_requests() {
    use crate::diesel_schema::print_attempts;
    use diesel::{ExpressionMethods as _, RunQueryDsl as _};
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    must(
        write(&f, move |tx| {
            diesel::update(print_attempts::table)
                .set(print_attempts::lease_expires_at_unix_ms.eq(now - 1))
                .execute(tx.connection())?;
            Ok(())
        })
        .await,
    );
    let stop = tokio_util::sync::CancellationToken::new();
    let (ready, _initialized) = tokio::sync::oneshot::channel();
    let maintenance = f.print.maintain_attempts(stop.clone(), ready);
    tokio::pin!(maintenance);
    let observation = async {
        loop {
            let identity = c.clone();
            if must(write(&f, move |tx| store::current(tx, &identity)).await).is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::select! {
            result = &mut maintenance => panic!("maintenance stopped before cancellation: {result:?}"),
            () = observation => {}
        }
    })
    .await;
    stop.cancel();
    must(maintenance.await);
    assert!(result.is_ok());
    assert_eq!(
        must(report(&f, &c, &a, AttemptReport::Contact, now + 1).await).state,
        AttemptState::Unknown
    );
}

#[tokio::test]
async fn background_tasks_expire_attempts_and_shutdown_stops_maintenance() {
    use crate::diesel_schema::print_attempts;
    use diesel::{ExpressionMethods as _, RunQueryDsl as _};
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let first = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    must(
        write(&f, move |tx| {
            diesel::update(print_attempts::table)
                .set(print_attempts::lease_expires_at_unix_ms.eq(now - 1))
                .execute(tx.connection())?;
            Ok(())
        })
        .await,
    );
    let state = std::sync::Arc::new(must(crate::server_state::tests::for_test(
        f.database.clone(),
        f.documents_path(),
    )));
    let weak_state = std::sync::Arc::downgrade(&state);
    let background = must(crate::background::start_background_tasks(&state).await);
    drop(state);
    assert!(weak_state.upgrade().is_none());
    let service = async {
        loop {
            let identity = c.clone();
            if must(write(&f, move |tx| store::current(tx, &identity)).await).is_none() {
                break Err::<(), _>(PrintError::Conflict);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    let result = must(tokio::time::timeout(std::time::Duration::from_secs(5), service).await);
    must(background.shutdown().await);
    assert_eq!(result, Err(PrintError::Conflict));
    let identity = c.clone();
    let id = first.attempt_id;
    assert_eq!(
        must(write(&f, move |tx| store::attempt(tx, &identity, &id)).await).state,
        AttemptState::Unknown
    );
    let second = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("second claim"));
    must(
        write(&f, move |tx| {
            diesel::update(print_attempts::table)
                .set(print_attempts::lease_expires_at_unix_ms.eq(now - 1))
                .execute(tx.connection())?;
            Ok(())
        })
        .await,
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    let identity = c.clone();
    let id = second.attempt_id;
    assert_eq!(
        must(write(&f, move |tx| store::attempt(tx, &identity, &id)).await).state,
        AttemptState::Preparing
    );
}

#[tokio::test]
async fn background_shutdown_does_not_end_active_print_attempts() {
    let f = Fixture::new().await;
    let client = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let attempt = claim(&f, &client, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let state = must(crate::server_state::tests::for_test(
        f.database.clone(),
        f.documents_path(),
    ));
    let background = must(crate::background::start_background_tasks(&state).await);
    must(background.shutdown().await);
    assert_eq!(
        must(write(&f, move |tx| store::current(tx, &client)).await),
        Some(attempt)
    );
}

#[tokio::test]
async fn assigned_document_is_verified_before_bytes_are_returned() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    assert!(
        !must(
            f.print
                .attempt_document(c.clone(), a.attempt_id.clone())
                .await
        )
        .is_empty()
    );
    must(std::fs::write(
        f.documents_path()
            .join(format!("{}.pdf", a.document_sha256)),
        b"corrupt fixture",
    ));
    assert_eq!(
        f.print.attempt_document(c, a.attempt_id).await,
        Err(PrintError::Persistence)
    );
    assert_eq!(must(f.print.list_executions().await)[0].attempts_used, 1);
}

#[tokio::test]
async fn admission_snapshots_count_live_work_once_and_remain_immutable() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let a = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let input = registration();
    let id = input.request_id.clone();
    must(f.print.register(f.source.clone(), input).await);
    let admitted = must(f.print.upload(f.source.clone(), id.clone(), pdf(2)).await);
    let snapshot = admitted
        .admission
        .as_ref()
        .unwrap_or_else(|| panic!("snapshot"));
    assert_eq!(
        (
            snapshot.ahead_tasks,
            snapshot.ahead_pages,
            snapshot.in_progress_tasks
        ),
        (0, 0, 1)
    );
    must(report(&f, &c, &a, AttemptReport::Failed, now + 1).await);
    assert_eq!(must(f.print.get(f.source.clone(), id).await), admitted);
}

#[tokio::test]
async fn restart_grace_only_extends_registered_active_jobs_and_preserves_terminal_results() {
    let f = Fixture::new().await;
    let c = client(&f).await;
    let other = client(&f).await;
    ready(&f).await;
    ready(&f).await;
    let now = must(write(&f, super::super::db::printers::now).await);
    let registered = claim(&f, &c, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let unregistered = claim(&f, &other, &Uuid::now_v7().to_string(), now)
        .await
        .unwrap_or_else(|| panic!("claim"));
    let identity = c.clone();
    let id = registered.attempt_id.clone();
    let j = job(&registered);
    must(
        write(&f, move |tx| {
            store::register_job(tx, &identity, &id, &j, now + 1)
        })
        .await,
    );
    let observed = must(
        report(
            &f,
            &c,
            &registered,
            AttemptReport::Observed(job(&registered)),
            now + 2,
        )
        .await,
    );
    must(write(&f, move |tx| store::recover(tx, now + 600_000)).await);
    let identity = c.clone();
    let recovered = must(write(&f, move |tx| store::current(tx, &identity)).await)
        .unwrap_or_else(|| panic!("registered job"));
    assert_eq!(recovered.lease_expires_at_unix_ms, now + 660_000);
    assert_eq!(
        recovered.last_observed_at_unix_ms,
        observed.last_observed_at_unix_ms
    );
    assert_eq!(
        recovered.last_contact_at_unix_ms,
        observed.last_contact_at_unix_ms
    );
    assert_eq!(
        must(
            report(
                &f,
                &other,
                &unregistered,
                AttemptReport::Contact,
                now + 600_001
            )
            .await
        )
        .state,
        AttemptState::Unknown
    );
    must(write(&f, move |tx| store::expire(tx, now + 660_000)).await);
    must(write(&f, move |tx| store::recover(tx, now + 700_000)).await);
    let identity = c.clone();
    assert!(must(write(&f, move |tx| store::current(tx, &identity)).await).is_none());
}
