use super::*;

#[tokio::test]
async fn normal_claim_submit_observe_and_report_is_one_native_execution() {
    let server = https(vec![
        Reply::Json(Value::Null),
        Reply::Json(json!(attempt())),
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Json(json!(registered())),
        Reply::Json(json!(registered())),
        Reply::Json(json!(terminal(AttemptState::Printed))),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    must(printer.observations.lock()).push_back(JobObservation::SpoolerReportedPrinted);
    let submissions = Arc::clone(&printer.submissions);
    let mut client = engine(&config, Work::Idle, false, printer);
    must(client.step(false).await);
    assert!(matches!(
        must(locked(&client.store)).state.work,
        Work::Preparing { .. }
    ));
    let claim: Value = must(std::fs::read_to_string(
        config.state_directory.join("state.jsonl"),
    ))
    .lines()
    .map(|line| must(serde_json::from_str::<Value>(line)))
    .find(|record| record["state"]["work"]["phase"] == "claiming")
    .unwrap_or(Value::Null);
    assert_eq!(
        claim["state"]["work"]["request_id"],
        must(server.seen.lock())[1].body["request_id"]
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    must(client.step(false).await);
    assert!(matches!(
        must(locked(&client.store)).state.work,
        Work::Reporting {
            report: Report::Printed(_),
            ..
        }
    ));
    must(client.step(false).await);
    assert_eq!(must(locked(&client.store)).state.work, Work::Idle);
    assert_eq!(submissions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_server_attempt_missing_locally_stops_new_claims() {
    let server = https(vec![Reply::Json(json!(registered()))]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut client = engine(&config, Work::Idle, false, FakePrinter::default());
    assert!(matches!(
        client.step(false).await,
        Err(ClientError::Attention { .. })
    ));
    assert_eq!(must(server.seen.lock()).len(), 1);
}

#[tokio::test]
async fn a_terminal_claim_receipt_is_not_executed_again() {
    let server = https(vec![Reply::Json(json!(terminal(AttemptState::Printed)))]).await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let mut client = engine(
        &config,
        Work::Claiming {
            request_id: Uuid::now_v7().to_string(),
        },
        false,
        FakePrinter::default(),
    );
    must(client.step(false).await);
    assert_eq!(must(locked(&client.store)).state.work, Work::Idle);
}

#[tokio::test]
async fn losing_registration_response_retries_only_the_same_windows_job() {
    let server = https(vec![
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Lost,
        Reply::Json(json!(registered())),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    let submissions = Arc::clone(&printer.submissions);
    let pages = Arc::clone(&printer.pages);
    let mut client = engine(
        &config,
        Work::Preparing { attempt: attempt() },
        false,
        printer,
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    assert!(pages.load(Ordering::SeqCst));
    assert_eq!(submissions.load(Ordering::SeqCst), 1);
    let seen = must(server.seen.lock());
    assert_eq!(seen[2].body, seen[3].body);
    assert_eq!(seen[2].body["job_id"], 42);
}

#[tokio::test]
async fn lost_ownership_before_registration_never_sends_a_page() {
    let server = https(vec![
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Json(json!(terminal(AttemptState::Unknown))),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    let pages = Arc::clone(&printer.pages);
    let mut client = engine(
        &config,
        Work::Preparing { attempt: attempt() },
        false,
        printer,
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    assert!(!pages.load(Ordering::SeqCst));
    assert!(matches!(
        must(locked(&client.store)).state.work,
        Work::Reporting {
            report: Report::Unknown,
            ..
        }
    ));
}

#[tokio::test]
async fn another_program_taking_the_queue_does_not_create_a_second_attempt() {
    let server = https(vec![
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Json(json!(registered())),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    printer.busy_once.store(true, Ordering::SeqCst);
    let submissions = Arc::clone(&printer.submissions);
    let mut client = engine(
        &config,
        Work::Preparing { attempt: attempt() },
        false,
        printer,
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    assert!(
        matches!(must(locked(&client.store)).state.work, Work::Preparing { ref attempt } if attempt.prepare_expires_at_unix_ms == 120_001)
    );
    must(client.step(false).await);
    must(submission_finished(&mut client).await);
    assert_eq!(submissions.load(Ordering::SeqCst), 1);
    assert!(
        must(server.seen.lock())
            .iter()
            .all(|request| !request.path.ends_with("/claims"))
    );
}

#[tokio::test]
async fn matching_jobs_are_observed_while_submission_is_still_running() {
    let server = https(vec![
        Reply::Json(json!(attempt())),
        Reply::Pdf(PDF.to_vec()),
        Reply::Json(json!(registered())),
        Reply::Json(json!(registered())),
        Reply::Json(json!(registered())),
    ])
    .await;
    let directory = must(tempfile::tempdir());
    let config = config(&directory, &server);
    let printer = FakePrinter::default();
    must(printer.observations.lock()).push_back(JobObservation::SpoolerReportedPrinted);
    let (release, released) = std::sync::mpsc::channel();
    *must(printer.release.lock()) = Some(released);
    let pages = Arc::clone(&printer.pages);
    let mut client = engine(
        &config,
        Work::Preparing { attempt: attempt() },
        false,
        printer,
    );
    must(client.step(false).await);
    must(
        tokio::time::timeout(Duration::from_secs(5), async {
            while must(server.seen.lock()).len() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await,
    );
    must(client.step(false).await);
    assert!(client.busy());
    assert!(!pages.load(Ordering::SeqCst));
    assert_eq!(must(server.seen.lock())[4].body["kind"], "observed");
    must(release.send(()));
    must(submission_finished(&mut client).await);
}

#[tokio::test]
async fn nullable_server_fields_are_required_and_old_version_fields_are_rejected() {
    for field in ["job_id", "last_observed_at_unix_ms", "finished_at_unix_ms"] {
        let mut value = json!(attempt());
        must(value.as_object_mut().ok_or("expected object")).remove(field);
        assert!(serde_json::from_value::<Attempt>(value).is_err());
    }
    let mut value = json!(attempt());
    value["protocol_version"] = json!(1);
    assert!(serde_json::from_value::<Attempt>(value).is_err());
}

#[tokio::test]
async fn journal_damage_never_falls_back_to_a_previous_idle_record() {
    for tail in [b"{\"sequence\":".as_slice(), b"\n".as_slice()] {
        let server = https(Vec::new()).await;
        let directory = must(tempfile::tempdir());
        let config = config(&directory, &server);
        let client = engine(&config, Work::Idle, false, FakePrinter::default());
        drop(client);
        let path = config.state_directory.join("state.jsonl");
        let mut file = must(std::fs::OpenOptions::new().append(true).open(&path));
        must(std::io::Write::write_all(&mut file, tail));
        let before = must(std::fs::read(&path));
        assert!(Store::open(&config).is_err());
        assert_eq!(must(std::fs::read(path)), before);
    }
}
