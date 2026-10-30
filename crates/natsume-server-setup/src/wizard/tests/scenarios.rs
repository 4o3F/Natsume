use super::*;
use super::{fake_host::Call, script::Selection};
use crate::{
    cli::ProbeCommand,
    health::HealthError,
    interaction::{PromptId, notice::Notice},
    probe::ProbeExit,
    process::BootstrapExit,
    service::ServiceAction,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

fn success_calls(bootstrap: bool) -> Vec<Call> {
    let mut calls = vec![
        Call::Preflight,
        Call::Probe(ProbeCommand::Deployment),
        Call::Probe(ProbeCommand::Bootstrap),
    ];
    if bootstrap {
        calls.push(Call::Bootstrap);
    }
    calls.extend([
        Call::Systemctl(ServiceAction::DaemonReload),
        Call::Systemctl(ServiceAction::Enable),
        Call::Systemctl(ServiceAction::Start),
        Call::Health(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8443)),
    ]);
    calls
}

fn tls_selection(script: &ScriptedInteraction) -> &Selection {
    let Some(selection) = script
        .selections
        .iter()
        .find(|selection| selection.id == PromptId::ServerTlsSource)
    else {
        panic!("Server TLS source was not prompted")
    };
    selection
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);
impl io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Ok(mut capture) = self.0.lock() else {
            panic!("log capture lock poisoned")
        };
        capture.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn fresh_generation_external_retention_and_log_canary() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    let mut script = generated_script(false);
    let log = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&log);
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || LogWriter(Arc::clone(&capture)))
        .finish();
    assert!(
        tracing::subscriber::set_global_default(subscriber).is_ok(),
        "test binary must have only one global tracing subscriber"
    );
    let outcome = fixture.run(&mut script)?;
    let WizardOutcome::Installed(summary) = outcome else {
        panic!("expected successful installation");
    };
    fixture.assert_installed()?;
    assert_public_handoff(&summary)?;
    assert_eq!(summary.control_key, KeyRetention::ExternalCopy);
    assert_eq!(summary.origin_key, KeyRetention::ExternalCopy);
    assert_eq!(files_under(&fixture.path("/admin/pki"))?.len(), 5);
    assert_eq!((script.reveals, script.clears), (2, 2));
    assert_eq!(fixture.host.calls, success_calls(true));
    assert!(
        script
            .defaults
            .contains(&(PromptId::HttpsPort, "8443".into()))
    );
    assert!(
        script
            .defaults
            .contains(&(PromptId::GatewayNotAfterTime, "00:00".into()))
    );
    let Ok(log) = log.lock() else {
        panic!("log capture lock poisoned")
    };
    let log = String::from_utf8_lossy(&log);
    assert!(
        log.contains("Server installed successfully"),
        "canary must capture real installation logs"
    );
    assert!(!log.contains(PASSPHRASE), "passphrase leaked into tracing");
    assert!(
        !log.contains("PRIVATE KEY"),
        "private-key export leaked into tracing"
    );
    assert!(
        matches!(script.notices.iter().find(|notice| matches!(notice, Notice::PlanSummary { .. })),
        Some(Notice::PlanSummary { entries, .. }) if entries.len() == 12)
    );
    Ok(())
}

#[test]
fn generated_keys_saved_encrypted_with_private_modes() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::DatabaseMissing);
    let mut script = generated_script(true);
    let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
        panic!("expected installation");
    };
    fixture.assert_installed()?;
    assert_public_handoff(&summary)?;
    let passphrase = SecretString::from(PASSPHRASE);
    for (name, trust) in [
        ("control-ca-key.pem", config::CONTROL_CA_TRUST),
        ("origin-ca-key.pem", config::LOCAL_ORIGIN_TRUST),
    ] {
        let path = fixture.path("/admin/pki").join(name);
        assert_eq!(fs::metadata(&path)?.mode() & 0o7777, 0o600);
        let key = import::import_private_key(&fs::read(path)?, Some(&passphrase))?;
        let certificate = import::import_certificate(&fs::read(fixture.path(trust))?)?;
        assert!(
            key.matches(&certificate),
            "saved key must match its authority"
        );
    }
    assert_eq!(
        fs::metadata(fixture.path("/admin/pki"))?.mode() & 0o7777,
        0o700
    );
    assert_eq!(files_under(&fixture.path("/admin/pki"))?.len(), 7);
    assert_eq!(fixture.host.calls, success_calls(true));
    Ok(())
}

#[test]
fn abort_phrase_removes_a_previously_saved_key_without_system_writes() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut prompts = generated_prefix();
    prompts.extend([
        (
            PromptId::KeyPersistence {
                role: CaRole::Control,
            },
            Reply::Confirm(true),
        ),
        (
            PromptId::KeyPersistence {
                role: CaRole::Origin,
            },
            Reply::Confirm(false),
        ),
        (
            PromptId::KeyRetentionConfirmation {
                role: CaRole::Origin,
            },
            Reply::text("abort"),
        ),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert_eq!((script.reveals, script.clears), (2, 2));
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    assert!(script.notices.iter().any(|notice| matches!(
        notice,
        Notice::RollbackReport {
            revoke_revealed: true,
            ..
        }
    )));
    Ok(())
}

fn import_control(prompts: &mut Vec<(PromptId, Reply)>, with_key: bool) {
    prompts.extend([
        (
            PromptId::ControlCaSource,
            Reply::Select(if with_key { 1 } else { 2 }),
        ),
        (
            PromptId::ControlCaCertificatePath,
            Reply::text("control.crt"),
        ),
    ]);
    if with_key {
        prompts.push((PromptId::ControlCaKeyPath, Reply::text("control.key")));
    }
}

fn origin_and_confirm(prompts: &mut Vec<(PromptId, Reply)>) {
    prompts.extend([
        (PromptId::OriginCaSource, Reply::Select(0)),
        (
            PromptId::NewCaPassphrase {
                role: CaRole::Origin,
            },
            Reply::secret(PASSPHRASE),
        ),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
        (
            PromptId::KeyPersistence {
                role: CaRole::Origin,
            },
            Reply::Confirm(false),
        ),
        (
            PromptId::KeyRetentionConfirmation {
                role: CaRole::Origin,
            },
            Reply::text("key saved"),
        ),
    ]);
}

#[test]
fn encrypted_control_import_issues_leaf_and_retries_wrong_passphrase() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Imported Control", "Natsume", fixture.host.now)?;
    let encrypted = generate::encrypt_private_key_pem(&key, &SecretString::from(PASSPHRASE))?;
    fixture.host.input("control.crt", ca.to_pem().as_bytes());
    fixture.host.input("control.key", encrypted.as_bytes());
    fixture.host.install_probes(ProbeExit::Passed);
    let mut prompts = parameters();
    import_control(&mut prompts, true);
    prompts.extend([
        (
            PromptId::ControlCaKeyPassphrase,
            Reply::secret("wrong canary password"),
        ),
        (PromptId::ControlCaKeyPassphrase, Reply::secret(PASSPHRASE)),
        (PromptId::ServerTlsSource, Reply::Select(0)),
    ]);
    origin_and_confirm(&mut prompts);
    let mut script = ScriptedInteraction::new(prompts);
    let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
        panic!("expected installation");
    };
    fixture.assert_installed()?;
    assert_public_handoff(&summary)?;
    assert_eq!(summary.control_key, KeyRetention::Imported);
    assert_eq!(script.reveals, 1);
    assert!(
        script
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::ValidationFailed { .. }))
    );
    assert!(
        script
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::BootstrapSkipped))
    );
    assert!(!fixture.path("/admin/pki/control-ca-key.pem").exists());
    let mut expected = success_calls(false);
    expected.splice(
        1..1,
        [
            Call::Read("control.crt".into()),
            Call::Read("control.key".into()),
        ],
    );
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn sec1_control_and_leaf_imports_are_used_without_copying_original_keys() -> TestResult {
    use pkcs8::{
        PrivateKeyInfoRef,
        der::{Decode as _, Encode as _},
    };
    let mut fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Imported Control", "Natsume", fixture.host.now)?;
    let (leaf, leaf_key) = generate::issue_server_leaf(
        &ca,
        &key,
        &[Ipv4Addr::LOCALHOST.into()],
        fixture.host.now + time::Duration::days(2),
        fixture.host.now,
    )?;
    for (path, key) in [("control.key", &key), ("leaf.key", &leaf_key)] {
        let info = PrivateKeyInfoRef::from_der(key.pkcs8_der())?;
        let mut ec = sec1::EcPrivateKey::from_der(info.private_key.as_bytes())?;
        let Some(parameters) = info.algorithm.parameters else {
            panic!("ECDSA algorithm must include named curve")
        };
        ec.parameters = Some(sec1::EcParameters::NamedCurve(parameters.decode_as()?));
        let sec1 =
            pkcs8::der::pem::encode_string("EC PRIVATE KEY", pkcs8::LineEnding::LF, &ec.to_der()?)?;
        fixture.host.input(path, sec1.as_bytes());
    }
    fixture.host.input("control.crt", ca.der());
    fixture.host.input("leaf.crt", leaf.der());
    fixture.host.install_probes(ProbeExit::Passed);
    let mut prompts = parameters();
    import_control(&mut prompts, true);
    prompts.extend([
        (PromptId::ServerTlsSource, Reply::Select(1)),
        (PromptId::ServerTlsCertificatePath, Reply::text("leaf.crt")),
        (PromptId::ServerTlsKeyPath, Reply::text("leaf.key")),
    ]);
    origin_and_confirm(&mut prompts);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::Installed(_)
    ));
    fixture.assert_installed()?;
    assert_eq!(files_under(&fixture.path("/admin/pki"))?.len(), 4);
    let mut expected = success_calls(false);
    expected.splice(
        1..1,
        [
            Call::Read("control.crt".into()),
            Call::Read("control.key".into()),
            Call::Read("leaf.crt".into()),
            Call::Read("leaf.key".into()),
        ],
    );
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

fn pending_first_run(fixture: &mut Fixture, ca: &Certificate) -> TestResult {
    fixture.host.input("control.crt", ca.der());
    let mut prompts = parameters();
    import_control(&mut prompts, false);
    prompts.extend([
        (PromptId::ServerTlsSource, Reply::Select(2)),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    let outcome = fixture.run(&mut script)?;
    assert!(
        matches!(outcome, WizardOutcome::PendingSignature { csr_path } if csr_path == fixture.path("/admin/pki/pending/server-tls.csr"))
    );
    fixture.assert_no_deployment()?;
    let files = files_under(&fixture.path("/admin/pki/pending"))?;
    let mut names: Vec<_> = files
        .iter()
        .map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "control-ca.crt",
            "request.toml",
            "server-tls-key.pk8",
            "server-tls.csr"
        ]
    );
    for path in files {
        assert_eq!(fs::metadata(path)?.mode() & 0o7777, 0o600);
    }
    let tls = tls_selection(&script);
    assert_eq!(tls.default, 2);
    assert!(
        tls.choices
            .first()
            .is_some_and(|choice| choice.1.as_deref() == Some("Control CA private key is offline"))
    );
    assert_eq!(script.reveals, 0);
    assert_eq!(
        fixture.host.calls,
        [Call::Preflight, Call::Read("control.crt".into())]
    );
    assert!(script.notices.iter().any(|notice| matches!(notice, Notice::PlanSummary { pending_only: true, entries, .. } if entries.len() == 1)));
    Ok(())
}

#[test]
fn certificate_only_defaults_to_csr_and_writes_only_four_pending_files() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, _) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    pending_first_run(&mut fixture, &ca)
}

#[test]
fn csr_publication_interrupt_aborts_and_removes_new_pending_without_system_writes() -> TestResult {
    for published in 0..4 {
        let mut fixture = Fixture::new()?;
        let (ca, _) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
        fixture.host.input("control.crt", ca.der());
        let mut prompts = parameters();
        import_control(&mut prompts, false);
        prompts.extend([
            (PromptId::ServerTlsSource, Reply::Select(2)),
            (PromptId::PlanConfirmation, Reply::Confirm(true)),
        ]);
        let mut script = ScriptedInteraction::new(prompts);
        let _interrupt = crate::deployment::file::tests::interrupt_publication_after(
            published,
            std::sync::Arc::clone(&fixture.host.interrupted),
        );
        assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
        assert!(fixture.host.interrupted());
        assert!(!fixture.path("/admin/pki/pending").exists());
        assert!(
            !script
                .notices
                .iter()
                .any(|notice| matches!(notice, Notice::OfflineSigningInstructions { .. }))
        );
        fixture.assert_no_deployment()?;
        assert!(files_under(&fixture.path("/etc"))?.is_empty());
        assert!(files_under(&fixture.path("/var/lib"))?.is_empty());
        assert!(!fixture.path(config::KEYS_DIRECTORY).exists());
        assert_eq!(
            fixture.host.calls,
            [Call::Preflight, Call::Read("control.crt".into())]
        );
    }
    Ok(())
}

#[test]
fn csr_instructions_interrupt_aborts_and_removes_new_pending() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, _) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    fixture.host.input("control.crt", ca.der());
    let mut prompts = parameters();
    import_control(&mut prompts, false);
    prompts.extend([
        (PromptId::ServerTlsSource, Reply::Select(2)),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    script.signing_interrupt = Some(std::sync::Arc::clone(&fixture.host.interrupted));
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert!(!fixture.path("/admin/pki/pending").exists());
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/var/lib"))?.is_empty());
    Ok(())
}

#[test]
fn csr_resume_interrupt_preserves_original_pending() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    pending_first_run(&mut fixture, &ca)?;
    let paths = files_under(&fixture.path("/admin/pki/pending"))?;
    let before = paths.iter().map(fs::read).collect::<Result<Vec<_>, _>>()?;
    let signed = sign_pending(&fixture, &ca, &key, false)?;
    fixture.host.input("signed.crt", signed.der());
    fixture.host.interrupt_after = Some(Call::Read("signed.crt".into()));
    fixture.host.calls.clear();
    let mut script = ScriptedInteraction::new(vec![(
        PromptId::SignedCertificatePath,
        Reply::text("signed.crt"),
    )]);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert_eq!(files_under(&fixture.path("/admin/pki/pending"))?, paths);
    let after = paths.iter().map(fs::read).collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        after, before,
        "resumed pending files must remain byte-for-byte unchanged"
    );
    fixture.assert_no_deployment()?;
    assert_eq!(
        fixture.host.calls,
        [Call::Preflight, Call::Read("signed.crt".into())]
    );
    Ok(())
}

fn sign_pending(
    fixture: &Fixture,
    ca: &Certificate,
    key: &PrivateKey,
    omit_ip: bool,
) -> Result<Certificate, TestError> {
    let pem = fs::read_to_string(fixture.path("/admin/pki/pending/server-tls.csr"))?;
    let mut request = rcgen::CertificateSigningRequestParams::from_pem(&pem)?;
    request.params.not_before = fixture.host.now - time::Duration::hours(1);
    request.params.not_after = fixture.host.now + time::Duration::days(30);
    if omit_ip {
        request.params.subject_alt_names.clear();
    }
    let issuer = rcgen::Issuer::from_ca_cert_der(
        &rustls_pki_types::CertificateDer::from(ca.der()),
        rcgen::KeyPair::try_from(&rustls_pki_types::PrivatePkcs8KeyDer::from(key.pkcs8_der()))?,
    )?;
    Ok(import::import_certificate(
        request.signed_by(&issuer)?.der(),
    )?)
}

#[test]
fn csr_resume_rejects_wrong_issuer_missing_ip_wrong_key_then_installs_and_removes_pending()
-> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    pending_first_run(&mut fixture, &ca)?;
    let valid = sign_pending(&fixture, &ca, &key, false)?;
    let missing_ip = sign_pending(&fixture, &ca, &key, true)?;
    let (other_ca, other_key) = generate::generate_ca("Wrong issuer", "Natsume", fixture.host.now)?;
    let wrong_issuer = sign_pending(&fixture, &other_ca, &other_key, false)?;
    let (wrong_key, _) = generate::issue_server_leaf(
        &ca,
        &key,
        &[Ipv4Addr::LOCALHOST.into()],
        fixture.host.now + time::Duration::days(2),
        fixture.host.now,
    )?;
    for (name, certificate) in [
        ("wrong-issuer.crt", wrong_issuer),
        ("missing-ip.crt", missing_ip),
        ("wrong-key.crt", wrong_key),
        ("valid.crt", valid),
    ] {
        fixture.host.input(name, certificate.der());
    }
    fixture.host.calls.clear();
    fixture.host.install_probes(ProbeExit::NotInitialized);
    let mut prompts: Vec<_> = [
        "wrong-issuer.crt",
        "missing-ip.crt",
        "wrong-key.crt",
        "valid.crt",
    ]
    .into_iter()
    .map(|path| (PromptId::SignedCertificatePath, Reply::text(path)))
    .collect();
    origin_and_confirm(&mut prompts);
    let mut script = ScriptedInteraction::new(prompts);
    let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
        panic!("expected resumed installation");
    };
    fixture.assert_installed()?;
    assert_public_handoff(&summary)?;
    assert!(!fixture.path("/admin/pki/pending").exists());
    assert_eq!(script.reveals, 1);
    assert_eq!(
        script
            .notices
            .iter()
            .filter(|notice| matches!(notice, Notice::ValidationFailed { .. }))
            .count(),
        3
    );
    let mut expected = success_calls(true);
    expected.splice(
        1..1,
        [
            "wrong-issuer.crt",
            "missing-ip.crt",
            "wrong-key.crt",
            "valid.crt",
        ]
        .map(|path| Call::Read(path.into())),
    );
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn csr_resume_escape_aborts_and_preserves_pending() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, _) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    pending_first_run(&mut fixture, &ca)?;
    fixture.host.calls.clear();
    let mut script = ScriptedInteraction::new(vec![(PromptId::SignedCertificatePath, Reply::Back)]);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    fixture.assert_no_deployment()?;
    assert_eq!(files_under(&fixture.path("/admin/pki/pending"))?.len(), 4);
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    Ok(())
}

#[test]
fn generated_control_disables_import_and_signing_request() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut prompts = parameters();
    prompts.extend([
        (PromptId::ControlCaSource, Reply::Select(0)),
        (PromptId::ServerTlsSource, Reply::Interrupt),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    let tls = tls_selection(&script);
    assert_eq!(tls.default, 0);
    let mut choices = tls.choices.iter();
    assert!(choices.next().is_some_and(|choice| choice.1.is_none()));
    assert!(choices.next().is_some_and(|choice| choice.1.is_some()));
    assert!(choices.next().is_some_and(|choice| choice.1.is_some()));
    fixture.assert_no_deployment()?;
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    Ok(())
}

#[test]
fn origin_p384_import_is_rejected_and_source_is_reasked() -> TestResult {
    let mut fixture = Fixture::new()?;
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384)?;
    let mut params = rcgen::CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    let ca = params.self_signed(&key)?;
    fixture.host.input("origin.crt", ca.der());
    fixture.host.input("origin.key", &key.serialize_der());
    let mut prompts = parameters();
    prompts.extend([
        (PromptId::ControlCaSource, Reply::Select(0)),
        (PromptId::ServerTlsSource, Reply::Select(0)),
        (PromptId::OriginCaSource, Reply::Select(1)),
        (PromptId::OriginCaCertificatePath, Reply::text("origin.crt")),
        (PromptId::OriginCaKeyPath, Reply::text("origin.key")),
        (PromptId::OriginCaSource, Reply::Interrupt),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert!(script.notices.iter().any(
        |notice| matches!(notice, Notice::ValidationFailed { message } if message.contains("P-256"))
    ));
    fixture.assert_no_deployment()?;
    assert_eq!(
        fixture.host.calls,
        [
            Call::Preflight,
            Call::Read("origin.crt".into()),
            Call::Read("origin.key".into())
        ]
    );
    Ok(())
}

#[test]
fn existing_deployment_is_rejected_without_writes() -> TestResult {
    let mut fixture = Fixture::new()?;
    fs::create_dir_all(fixture.path(config::CONFIG_DIRECTORY))?;
    fs::write(fixture.path(config::CONFIG_PATH), b"existing deployment")?;
    let before = files_under(fixture.root.path())?;
    let mut script = ScriptedInteraction::new(Vec::new());
    assert!(matches!(
        fixture.run(&mut script),
        Err(WizardError::AlreadyDeployed)
    ));
    assert_eq!(files_under(fixture.root.path())?, before);
    assert_eq!(
        fs::read(fixture.path(config::CONFIG_PATH))?,
        b"existing deployment"
    );
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    Ok(())
}

#[test]
fn interruption_before_confirmation_leaves_no_files_at_each_prompt() -> TestResult {
    let prompts = generated_prefix();
    for stop in 0..prompts.len() {
        let mut fixture = Fixture::new()?;
        let mut prefix = generated_prefix();
        prefix.truncate(stop);
        let Some((id, _)) = prompts.get(stop) else {
            panic!("bounded prompt index")
        };
        let id = *id;
        prefix.push((id, Reply::Interrupt));
        let mut script = ScriptedInteraction::new(prefix);
        assert!(
            matches!(fixture.run(&mut script)?, WizardOutcome::Aborted),
            "interruption at {id:?}"
        );
        assert!(
            files_under(fixture.root.path())?.is_empty(),
            "interruption wrote files at {id:?}"
        );
        assert_eq!(fixture.host.calls, [Call::Preflight]);
    }
    Ok(())
}

#[test]
fn health_failure_rolls_back_but_preserves_bootstrap_state_and_rerun_skips_bootstrap() -> TestResult
{
    let mut fixture = Fixture::new()?;
    fixture.host.bootstrap_root = Some(fixture.root.path().into());
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.health = Some(Err(HealthError::Unhealthy));
    let mut script = generated_script(true);
    let Err(error) = fixture.run(&mut script) else {
        panic!("unhealthy Server must fail installation")
    };
    let WizardError::Failed {
        source: WizardFailure::Health { .. },
        rollback: Some(rollback),
    } = error
    else {
        panic!("expected original health error and cleanup records");
    };
    fixture.assert_no_deployment()?;
    assert!(
        rollback
            .files
            .kept
            .contains(&fixture.path(config::KEYS_DIRECTORY))
    );
    assert!(rollback.files.failures.is_empty());
    assert_eq!(
        rollback
            .files
            .removed
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "pk8"))
            .count(),
        2
    );
    for path in [config::DATABASE, config::ROOT_KEY] {
        assert_eq!(fs::read(fixture.path(path))?, b"bootstrap-owned-state");
    }
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    let mut expected = success_calls(true);
    expected.extend([
        Call::Systemctl(ServiceAction::Stop),
        Call::Systemctl(ServiceAction::Disable),
    ]);
    assert_eq!(fixture.host.calls, expected);
    fixture.host.calls.clear();
    fixture.host.install_probes(ProbeExit::Passed);
    let mut script = generated_script(false);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::Installed(_)
    ));
    fixture.assert_installed()?;
    assert!(
        script
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::BootstrapSkipped))
    );
    assert_eq!(fixture.host.calls, success_calls(false));
    Ok(())
}

#[test]
fn bootstrap_failure_reprobes_and_retries_then_succeeds() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.bootstraps = VecDeque::from([
        Ok(BootstrapExit::Failed { code: 1 }),
        Ok(BootstrapExit::Succeeded),
    ]);
    fixture
        .host
        .probes
        .push_back((ProbeCommand::Bootstrap, Ok(ProbeExit::NotInitialized)));
    let mut prompts = generated_prefix();
    prompts.extend(retention(false));
    prompts.push((PromptId::BootstrapRetry, Reply::Confirm(true)));
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::Installed(_)
    ));
    fixture.assert_installed()?;
    let mut expected = success_calls(true);
    expected.splice(
        4..4,
        [Call::Probe(ProbeCommand::Bootstrap), Call::Bootstrap],
    );
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn bootstrap_three_failures_roll_back_and_keep_original_process_source() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.bootstraps = VecDeque::from([
        Ok(BootstrapExit::Failed { code: 1 }),
        Ok(BootstrapExit::Failed { code: 1 }),
        Err(ProcessError::Spawn {
            program: "fixture bootstrap",
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        }),
    ]);
    for _ in 0..3 {
        fixture
            .host
            .probes
            .push_back((ProbeCommand::Bootstrap, Ok(ProbeExit::NotInitialized)));
    }
    let mut prompts = generated_prefix();
    prompts.extend(retention(false));
    prompts.extend([
        (PromptId::BootstrapRetry, Reply::Confirm(true)),
        (PromptId::BootstrapRetry, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    let Err(error) = fixture.run(&mut script) else {
        panic!("three failed bootstrap attempts must fail")
    };
    assert!(matches!(
        error,
        WizardError::Failed {
            source: WizardFailure::Process {
                source: ProcessError::Spawn { .. }
            },
            rollback: Some(_)
        }
    ));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert_eq!(
        fixture
            .host
            .calls
            .iter()
            .filter(|call| **call == Call::Bootstrap)
            .count(),
        3
    );
    assert_eq!(
        fixture.host.calls.last(),
        Some(&Call::Systemctl(ServiceAction::Stop))
    );
    assert!(
        !fixture
            .host
            .calls
            .contains(&Call::Systemctl(ServiceAction::Enable))
    );
    Ok(())
}

#[test]
fn failed_bootstrap_with_initialized_reprobe_is_accepted() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.bootstraps = VecDeque::from([Ok(BootstrapExit::Failed { code: 1 })]);
    fixture
        .host
        .probes
        .push_back((ProbeCommand::Bootstrap, Ok(ProbeExit::Passed)));
    let mut script = generated_script(false);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::Installed(_)
    ));
    fixture.assert_installed()?;
    let mut expected = success_calls(true);
    expected.insert(4, Call::Probe(ProbeCommand::Bootstrap));
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn invalid_parameters_reprompt_only_the_rejected_field() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut prompts = vec![
        (PromptId::ServerIps, Reply::text("")),
        (PromptId::ServerIps, Reply::text("127.0.0.1")),
        (PromptId::HttpsPort, Reply::text("80")),
        (PromptId::HttpsPort, Reply::text("8443")),
    ];
    prompts.extend(parameters().into_iter().skip(2).take(4));
    prompts.extend([
        (
            PromptId::GatewayNotAfterDate,
            Reply::Date(OffsetDateTime::parse(END, &Rfc3339)?.date()),
        ),
        (PromptId::GatewayNotAfterTime, Reply::text("00:00")),
        (
            PromptId::GatewayNotAfterDate,
            Reply::Date(OffsetDateTime::parse(EXPIRY, &Rfc3339)?.date()),
        ),
        (PromptId::GatewayNotAfterTime, Reply::text("00:00")),
        (PromptId::ControlCaSource, Reply::Interrupt),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert_eq!(
        script
            .notices
            .iter()
            .filter(|notice| matches!(notice, Notice::ValidationFailed { .. }))
            .count(),
        3
    );
    fixture.assert_no_deployment()?;
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    Ok(())
}

#[test]
fn escape_from_server_tls_reselects_control_and_clears_downstream_answers() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, _) = generate::generate_ca("Offline replacement", "Natsume", fixture.host.now)?;
    fixture.host.input("control.crt", ca.der());
    let mut prompts = parameters();
    prompts.extend([
        (PromptId::ControlCaSource, Reply::Select(0)),
        (PromptId::ServerTlsSource, Reply::Back),
    ]);
    import_control(&mut prompts, false);
    prompts.extend([
        (PromptId::ServerTlsSource, Reply::Select(2)),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::PendingSignature { .. }
    ));
    fixture.assert_no_deployment()?;
    let mut selections = script
        .selections
        .iter()
        .filter(|selection| selection.id == PromptId::ServerTlsSource);
    assert_eq!(
        selections.next().map(|selection| selection.default),
        Some(0)
    );
    assert_eq!(
        selections.next().map(|selection| selection.default),
        Some(2)
    );
    let Some(pending) = PendingRequest::load(
        &fixture.context.target_root,
        &fixture.context.pki_dir,
        fixture.host.now,
        fixture.context.root_owner,
    )?
    else {
        panic!("replacement CSR must exist")
    };
    assert_eq!(pending.control_ca(), &ca);
    assert_eq!(script.reveals, 0);
    assert_eq!(
        fixture.host.calls,
        [Call::Preflight, Call::Read("control.crt".into())]
    );
    Ok(())
}

#[test]
fn every_parameter_rejection_maps_to_its_prompt() {
    use crate::config::ParameterRejection;
    for (reason, prompt) in [
        (ParameterRejection::EmptyServerIps, PromptId::ServerIps),
        (ParameterRejection::TooManyServerIps, PromptId::ServerIps),
        (ParameterRejection::UnusableServerIp, PromptId::ServerIps),
        (ParameterRejection::HttpsPortOutOfRange, PromptId::HttpsPort),
        (
            ParameterRejection::EmptyGatewayHostname,
            PromptId::GatewayHostname,
        ),
        (
            ParameterRejection::EmptyDomjudgeOrigin,
            PromptId::DomjudgeOrigin,
        ),
        (
            ParameterRejection::ContestEndFormat,
            PromptId::ContestEndDate,
        ),
        (
            ParameterRejection::GatewayNotAfterFormat,
            PromptId::GatewayNotAfterDate,
        ),
        (
            ParameterRejection::GatewayValidityTooShort,
            PromptId::GatewayNotAfterDate,
        ),
        (
            ParameterRejection::GatewayExpired,
            PromptId::GatewayNotAfterDate,
        ),
    ] {
        assert_eq!(
            super::super::params::rejection_prompt(reason),
            prompt,
            "wrong prompt for {reason:?}"
        );
    }
}

#[test]
fn handoff_failure_keeps_the_installed_deployment_and_reports_manual_steps() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let mut fixture = Fixture::new()?;
    let occupied = fixture.path("/admin/pki/handoff/20290101T000000Z");
    fs::create_dir_all(&occupied)?;
    fs::set_permissions(
        fixture.path("/admin/pki"),
        fs::Permissions::from_mode(0o700),
    )?;
    fs::write(occupied.join("existing-export"), b"keep existing handoff")?;
    fixture.host.install_probes(ProbeExit::Passed);
    let mut script = generated_script(false);
    let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
        panic!("handoff failure must remain installed")
    };
    assert!(matches!(
        summary.handoff,
        Err(HandoffError::Existing { .. })
    ));
    fixture.assert_installed()?;
    assert_eq!(
        fs::read(occupied.join("existing-export"))?,
        b"keep existing handoff"
    );
    assert!(script.notices.iter().any(|notice| {
        matches!(
            notice,
            Notice::InstallationSummary {
                handoff_directory: None,
                ..
            }
        ) && notice
            .to_string()
            .contains("Manually create Client config.toml")
    }));
    assert_eq!(fixture.host.calls, success_calls(false));
    Ok(())
}

#[test]
fn interruption_during_bootstrap_retry_rolls_back_deployment_and_saved_keys() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.bootstraps = VecDeque::from([Ok(BootstrapExit::Failed { code: 1 })]);
    fixture
        .host
        .probes
        .push_back((ProbeCommand::Bootstrap, Ok(ProbeExit::NotInitialized)));
    let mut prompts = generated_prefix();
    prompts.extend(retention(true));
    prompts.push((PromptId::BootstrapRetry, Reply::Interrupt));
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert_eq!(
        fixture.host.calls,
        [
            Call::Preflight,
            Call::Probe(ProbeCommand::Deployment),
            Call::Probe(ProbeCommand::Bootstrap),
            Call::Bootstrap,
            Call::Probe(ProbeCommand::Bootstrap),
            Call::Systemctl(ServiceAction::Stop)
        ]
    );
    Ok(())
}

#[test]
fn service_failure_preserves_original_error_and_records_cleanup_failures() -> TestResult {
    use crate::service::ServiceError;
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::Passed);
    fixture.host.services = VecDeque::from([
        (ServiceAction::DaemonReload, Ok(())),
        (ServiceAction::Enable, Ok(())),
        (
            ServiceAction::Start,
            Err(ServiceError::Launch {
                action: ServiceAction::Start,
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            }),
        ),
        (
            ServiceAction::Stop,
            Err(ServiceError::Launch {
                action: ServiceAction::Stop,
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            }),
        ),
        (
            ServiceAction::Disable,
            Err(ServiceError::Launch {
                action: ServiceAction::Disable,
                source: io::Error::from(io::ErrorKind::PermissionDenied),
            }),
        ),
    ]);
    let mut script = generated_script(false);
    let Err(WizardError::Failed {
        source:
            WizardFailure::Service {
                source:
                    ServiceError::Launch {
                        action: ServiceAction::Start,
                        ..
                    },
            },
        rollback: Some(rollback),
    }) = fixture.run(&mut script)
    else {
        panic!("expected startup error with cleanup records")
    };
    assert_eq!(rollback.service_failures.len(), 2);
    assert!(rollback.files.failures.is_empty());
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    let mut expected = success_calls(false);
    expected.pop();
    expected.extend([
        Call::Systemctl(ServiceAction::Stop),
        Call::Systemctl(ServiceAction::Disable),
    ]);
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn imported_key_passphrase_stops_after_three_wrong_attempts_without_writes() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Imported Control", "Natsume", fixture.host.now)?;
    let encrypted = generate::encrypt_private_key_pem(&key, &SecretString::from(PASSPHRASE))?;
    fixture.host.input("control.crt", ca.der());
    fixture.host.input("control.key", encrypted.as_bytes());
    let mut prompts = parameters();
    import_control(&mut prompts, true);
    for _ in 0..3 {
        prompts.push((
            PromptId::ControlCaKeyPassphrase,
            Reply::secret("incorrect-secret-canary"),
        ));
    }
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(
        fixture.run(&mut script),
        Err(WizardError::Failed {
            source: WizardFailure::Import {
                source: ImportError::WrongPassphrase { .. }
            },
            rollback: None
        })
    ));
    assert!(files_under(fixture.root.path())?.is_empty());
    assert_eq!(
        fixture.host.calls,
        [
            Call::Preflight,
            Call::Read("control.crt".into()),
            Call::Read("control.key".into())
        ]
    );
    assert_eq!(script.reveals, 0);
    Ok(())
}

#[test]
fn warnings_require_explicit_confirmation_and_declined_plans_allow_edit_or_abort() -> TestResult {
    use crate::preflight::PreflightWarning;
    let mut fixture = Fixture::new()?;
    fixture
        .host
        .warnings
        .push(PreflightWarning::ClockNotSynchronized);
    let mut script = ScriptedInteraction::new(vec![(
        PromptId::ContinueAfterWarnings,
        Reply::Confirm(false),
    )]);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert!(files_under(fixture.root.path())?.is_empty());
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    for selection in [0, 1] {
        let mut fixture = Fixture::new()?;
        let mut prompts = generated_prefix();
        prompts.pop();
        prompts.extend([
            (PromptId::PlanConfirmation, Reply::Confirm(false)),
            (PromptId::PlanDeclined, Reply::Select(selection)),
        ]);
        if selection == 0 {
            prompts.push((PromptId::ServerIps, Reply::Interrupt));
        }
        let mut script = ScriptedInteraction::new(prompts);
        assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
        assert!(files_under(fixture.root.path())?.is_empty());
        assert_eq!(script.reveals, 0);
        assert_eq!(fixture.host.calls, [Call::Preflight]);
    }
    Ok(())
}

#[test]
fn certificate_only_expired_control_is_rejected_before_creating_pending() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, _) = generate::generate_ca(
        "Expired offline CA",
        "Natsume",
        fixture.host.now - time::Duration::days(3651),
    )?;
    fixture.host.input("control.crt", ca.der());
    let mut prompts = parameters();
    import_control(&mut prompts, false);
    prompts.push((PromptId::ControlCaSource, Reply::Interrupt));
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert!(files_under(fixture.root.path())?.is_empty());
    assert_eq!(
        fixture.host.calls,
        [Call::Preflight, Call::Read("control.crt".into())]
    );
    assert!(script.notices.iter().any(|notice| matches!(notice, Notice::ValidationFailed { message } if message.contains("valid now"))));
    Ok(())
}

#[test]
fn all_imported_materials_install_without_revealing_or_copying_original_keys() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (control, control_key) =
        generate::generate_ca("Imported Control", "Natsume", fixture.host.now)?;
    let (origin, origin_key) =
        generate::generate_ca("Imported Origin", "Natsume", fixture.host.now)?;
    let (leaf, leaf_key) = generate::issue_server_leaf(
        &control,
        &control_key,
        &[Ipv4Addr::LOCALHOST.into()],
        fixture.host.now + time::Duration::days(2),
        fixture.host.now,
    )?;
    for (path, bytes) in [
        ("control.crt", control.der()),
        ("control.key", control_key.pkcs8_der()),
        ("leaf.crt", leaf.der()),
        ("leaf.key", leaf_key.pkcs8_der()),
        ("origin.crt", origin.der()),
        ("origin.key", origin_key.pkcs8_der()),
    ] {
        fixture.host.input(path, bytes);
    }
    fixture.host.install_probes(ProbeExit::Passed);
    let mut prompts = parameters();
    import_control(&mut prompts, true);
    prompts.extend([
        (PromptId::ServerTlsSource, Reply::Select(1)),
        (PromptId::ServerTlsCertificatePath, Reply::text("leaf.crt")),
        (PromptId::ServerTlsKeyPath, Reply::text("leaf.key")),
        (PromptId::OriginCaSource, Reply::Select(1)),
        (PromptId::OriginCaCertificatePath, Reply::text("origin.crt")),
        (PromptId::OriginCaKeyPath, Reply::text("origin.key")),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
        panic!("expected imported installation")
    };
    fixture.assert_installed()?;
    assert_public_handoff(&summary)?;
    assert_eq!(summary.control_key, KeyRetention::Imported);
    assert_eq!(summary.origin_key, KeyRetention::Imported);
    assert_eq!((script.reveals, script.clears), (0, 0));
    assert_eq!(files_under(&fixture.path("/admin/pki"))?.len(), 4);
    let mut expected = success_calls(false);
    expected.splice(
        1..1,
        [
            "control.crt",
            "control.key",
            "leaf.crt",
            "leaf.key",
            "origin.crt",
            "origin.key",
        ]
        .map(|path| Call::Read(path.into())),
    );
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn summary_failure_after_health_keeps_installed_deployment_and_handoff() -> TestResult {
    for error in [
        InteractionError::Interrupted,
        InteractionError::Output {
            source: io::Error::from(io::ErrorKind::BrokenPipe),
        },
    ] {
        let mut fixture = Fixture::new()?;
        fixture.host.install_probes(ProbeExit::Passed);
        let mut script = generated_script(true);
        script.summary_error = Some(error);
        let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
            panic!("summary failure after health must remain installed")
        };
        fixture.assert_installed()?;
        assert_public_handoff(&summary)?;
        assert!(summary.summary_display.is_err());
        assert!(summary.pending_cleanup.is_ok());
        assert!(fixture.path("/admin/pki/control-ca-key.pem").is_file());
        assert!(fixture.path("/admin/pki/origin-ca-key.pem").is_file());
        assert_eq!(fixture.host.calls, success_calls(false));
    }
    Ok(())
}

#[test]
fn pending_cleanup_failure_after_health_keeps_installed_deployment_and_handoff() -> TestResult {
    let mut fixture = Fixture::new()?;
    let (ca, key) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    pending_first_run(&mut fixture, &ca)?;
    let valid = sign_pending(&fixture, &ca, &key, false)?;
    fixture.host.input("valid.crt", valid.der());
    fixture.host.calls.clear();
    fixture.host.install_probes(ProbeExit::Passed);
    let mut prompts = vec![(PromptId::SignedCertificatePath, Reply::text("valid.crt"))];
    origin_and_confirm(&mut prompts);
    let mut script = ScriptedInteraction::new(prompts);
    let changed = fixture.path("/admin/pki/pending/server-tls-key.pk8");
    script.summary_changed_file = Some(changed.clone());
    let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
        panic!("pending cleanup failure after health must remain installed")
    };
    fixture.assert_installed()?;
    assert_public_handoff(&summary)?;
    assert!(summary.summary_display.is_ok());
    assert!(matches!(
        summary.pending_cleanup,
        Err(PendingError::Filesystem { .. })
    ));
    assert_eq!(fs::read(changed)?, b"changed pending file");
    let mut expected = success_calls(false);
    expected.insert(1, Call::Read("valid.crt".into()));
    assert_eq!(fixture.host.calls, expected);
    Ok(())
}

#[test]
fn three_nonzero_bootstrap_exits_roll_back_and_preserve_the_last_exit_code() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.bootstraps = [1, 2, 7]
        .into_iter()
        .map(|code| Ok(BootstrapExit::Failed { code }))
        .collect();
    for _ in 0..3 {
        fixture
            .host
            .probes
            .push_back((ProbeCommand::Bootstrap, Ok(ProbeExit::NotInitialized)));
    }
    let mut prompts = generated_prefix();
    prompts.extend(retention(false));
    prompts.extend([
        (PromptId::BootstrapRetry, Reply::Confirm(true)),
        (PromptId::BootstrapRetry, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(
        fixture.run(&mut script),
        Err(WizardError::Failed {
            source: WizardFailure::Bootstrap { code: 7 },
            rollback: Some(_)
        })
    ));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert_eq!(
        fixture.host.calls,
        [
            Call::Preflight,
            Call::Probe(ProbeCommand::Deployment),
            Call::Probe(ProbeCommand::Bootstrap),
            Call::Bootstrap,
            Call::Probe(ProbeCommand::Bootstrap),
            Call::Bootstrap,
            Call::Probe(ProbeCommand::Bootstrap),
            Call::Bootstrap,
            Call::Probe(ProbeCommand::Bootstrap),
            Call::Systemctl(ServiceAction::Stop)
        ]
    );
    Ok(())
}

#[test]
fn escape_after_key_reveal_repeats_persistence_and_retention_prompts() -> TestResult {
    for save in [false, true] {
        let mut fixture = Fixture::new()?;
        fixture.host.install_probes(ProbeExit::Passed);
        let mut prompts = generated_prefix();
        for (id, reply) in retention(save) {
            prompts.extend([(id, Reply::Back), (id, Reply::Back), (id, reply)]);
        }
        let mut script = ScriptedInteraction::new(prompts);
        let WizardOutcome::Installed(summary) = fixture.run(&mut script)? else {
            panic!("Escape after reveal must allow completing the same prompt")
        };
        fixture.assert_installed()?;
        assert_public_handoff(&summary)?;
        if save {
            assert!(matches!(summary.control_key, KeyRetention::Saved(_)));
            assert!(matches!(summary.origin_key, KeyRetention::Saved(_)));
        } else {
            assert_eq!(summary.control_key, KeyRetention::ExternalCopy);
            assert_eq!(summary.origin_key, KeyRetention::ExternalCopy);
        }
        assert_eq!((script.reveals, script.clears), (2, 2));
        assert_eq!(fixture.host.calls, success_calls(false));
    }
    Ok(())
}

#[test]
fn escape_after_key_reveal_still_allows_explicit_abort() -> TestResult {
    let mut fixture = Fixture::new()?;
    let mut prompts = generated_prefix();
    let role = CaRole::Control;
    prompts.extend([
        (PromptId::KeyPersistence { role }, Reply::Back),
        (PromptId::KeyPersistence { role }, Reply::Confirm(false)),
        (PromptId::KeyRetentionConfirmation { role }, Reply::Back),
        (
            PromptId::KeyRetentionConfirmation { role },
            Reply::text("abort"),
        ),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert_eq!((script.reveals, script.clears), (1, 1));
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    Ok(())
}

#[test]
fn escape_or_decline_bootstrap_retry_preserves_original_failure() -> TestResult {
    for reply in [Reply::Back, Reply::Confirm(false)] {
        let mut fixture = Fixture::new()?;
        fixture.host.install_probes(ProbeExit::NotInitialized);
        fixture.host.bootstraps = VecDeque::from([Ok(BootstrapExit::Failed { code: 7 })]);
        fixture
            .host
            .probes
            .push_back((ProbeCommand::Bootstrap, Ok(ProbeExit::NotInitialized)));
        let mut prompts = generated_prefix();
        prompts.extend(retention(false));
        prompts.push((PromptId::BootstrapRetry, reply));
        let mut script = ScriptedInteraction::new(prompts);
        assert!(matches!(
            fixture.run(&mut script),
            Err(WizardError::Failed {
                source: WizardFailure::Bootstrap { code: 7 },
                rollback: Some(_)
            })
        ));
        fixture.assert_no_deployment()?;
        assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
        assert_eq!(
            fixture.host.calls,
            [
                Call::Preflight,
                Call::Probe(ProbeCommand::Deployment),
                Call::Probe(ProbeCommand::Bootstrap),
                Call::Bootstrap,
                Call::Probe(ProbeCommand::Bootstrap),
                Call::Systemctl(ServiceAction::Stop),
            ]
        );
    }
    Ok(())
}

#[test]
fn not_root_preflight_fails_once_without_prompting_or_writes() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.preflight = Some(Err(PreflightError::NotRoot));
    let mut script = ScriptedInteraction::new(Vec::new());
    assert!(matches!(
        fixture.run(&mut script),
        Err(WizardError::Failed {
            source: WizardFailure::Preflight {
                source: PreflightError::NotRoot
            },
            rollback: None,
        })
    ));
    assert_eq!(fixture.host.calls, [Call::Preflight]);
    assert!(script.notices.is_empty());
    assert!(files_under(fixture.root.path())?.is_empty());
    Ok(())
}

#[test]
fn interrupt_during_bootstrap_rolls_back() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::NotInitialized);
    fixture.host.interrupt_after = Some(Call::Bootstrap);
    let mut script = generated_script(true);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert!(
        fixture
            .host
            .calls
            .contains(&Call::Systemctl(ServiceAction::Stop))
    );
    Ok(())
}

#[test]
fn interrupt_during_health_rolls_back() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::Passed);
    fixture.host.interrupt_after = Some(Call::Health(SocketAddr::new(
        Ipv4Addr::LOCALHOST.into(),
        8443,
    )));
    let mut script = generated_script(true);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    assert!(
        fixture
            .host
            .calls
            .contains(&Call::Systemctl(ServiceAction::Disable))
    );
    Ok(())
}

#[test]
fn interrupt_after_commit_keeps_installed_deployment() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::Passed);
    let mut script = generated_script(true);
    script.summary_interrupt = Some(Arc::clone(&fixture.host.interrupted));
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::Installed(_)
    ));
    assert!(fixture.host.interrupted());
    fixture.assert_installed()?;
    assert!(
        !fixture
            .host
            .calls
            .contains(&Call::Systemctl(ServiceAction::Stop))
    );
    Ok(())
}

#[test]
fn validation_notices_preserve_safe_error_chains() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.input("/malformed", b"invalid certificate");
    let mut prompts = parameters();
    prompts.extend([
        (PromptId::ControlCaSource, Reply::Select(2)),
        (PromptId::ControlCaCertificatePath, Reply::text("/missing")),
        (
            PromptId::ControlCaCertificatePath,
            Reply::text("/malformed"),
        ),
        (PromptId::ControlCaCertificatePath, Reply::Interrupt),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    let messages: Vec<_> = script
        .notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::ValidationFailed { message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .first()
            .is_some_and(|message| message.contains("cannot read input file")
                && message.contains(&io::Error::from(io::ErrorKind::NotFound).to_string()))
    );
    assert!(messages.get(1).is_some_and(
        |message| message.contains("PKI import failed") && message.contains("certificate")
    ));
    fixture.assert_no_deployment()?;
    Ok(())
}

#[test]
fn publication_sync_failure_rolls_back_saved_pki() -> TestResult {
    for count in 0..3 {
        let mut fixture = Fixture::new()?;
        let _fault = crate::deployment::file::tests::fail_publication_sync_after(count);
        let mut prompts = generated_prefix();
        let mut saves = retention(true);
        if count == 0 {
            saves.truncate(1);
        }
        prompts.extend(saves);
        let mut script = ScriptedInteraction::new(prompts);
        let Err(error) = fixture.run(&mut script) else {
            panic!("PKI publication fsync fault must fail")
        };
        assert!(matches!(
            error,
            WizardError::Failed {
                source: WizardFailure::File {
                    source: FileError::PublishedNotDurable { .. }
                },
                ..
            }
        ));
        fixture.assert_no_deployment()?;
        assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
        assert!(script.notices.iter().any(|notice| matches!(notice, Notice::RollbackReport { failures, .. } if failures.is_empty())));
    }
    Ok(())
}

#[test]
fn publication_interrupt_preserves_apply_error_and_partial_rollback() -> TestResult {
    use crate::deployment::{apply::ApplyError, file::tests};
    let mut fixture = Fixture::new()?;
    let mut script = generated_script(true);
    let _interrupt = tests::interrupt_publication_after(9, Arc::clone(&fixture.host.interrupted));
    let _fault = tests::fail_publication_sync_after(9);
    tests::modify_next_rollback_file();
    let Err(WizardError::Failed {
        source: WizardFailure::Apply { source },
        rollback: Some(rollback),
    }) = fixture.run(&mut script)
    else {
        panic!("simultaneous cancellation must preserve the ApplyError and rollback results");
    };
    assert!(fixture.host.interrupted());
    assert!(matches!(
        source.as_ref(),
        ApplyError::CreateFile {
            source: FileError::PublishedNotDurable { .. },
            ..
        }
    ));
    assert_eq!(rollback.files.failures.len(), 1, "{rollback:?}");
    let failed = &rollback.files.failures[0];
    assert_eq!(failed.path, fixture.path(config::CONFIG_PATH));
    assert!(matches!(
        failed.source,
        FileError::ChangedSinceCreation { .. }
    ));
    assert!(
        rollback
            .files
            .removed
            .contains(&fixture.path(config::ORIGIN_CA_KEY))
    );
    assert_eq!(fs::read(&failed.path)?, b"modified during rollback");
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    let reports: Vec<_> = script
        .notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::RollbackReport { failures, .. } => Some(failures),
            _ => None,
        })
        .collect();
    assert_eq!(reports, [&vec![failed.path.clone()]]);
    assert_eq!(
        fixture.host.calls,
        [Call::Preflight, Call::Systemctl(ServiceAction::Stop)]
    );
    Ok(())
}

#[test]
fn health_failure_takes_precedence_over_simultaneous_interrupt() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.install_probes(ProbeExit::Passed);
    fixture.host.health = Some(Err(HealthError::Unhealthy));
    fixture.host.interrupt_after = Some(Call::Health(SocketAddr::new(
        Ipv4Addr::LOCALHOST.into(),
        8443,
    )));
    let mut script = generated_script(true);
    assert!(matches!(
        fixture.run(&mut script),
        Err(WizardError::Failed {
            source: WizardFailure::Health {
                source: HealthError::Unhealthy
            },
            rollback: Some(_)
        })
    ));
    fixture.assert_no_deployment()?;
    assert!(files_under(&fixture.path("/admin/pki"))?.is_empty());
    Ok(())
}

#[test]
fn local_plus_eight_renders_utc_and_csr_resume_preserves_parameters() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.offset = time::UtcOffset::from_hms(8, 0, 0)?;
    fixture.host.timezone.name = Some("Asia/Shanghai".into());
    let (ca, key) = generate::generate_ca("Offline Control", "Natsume", fixture.host.now)?;
    fixture.host.input("control.crt", ca.der());
    let mut prompts: Vec<_> = parameters()
        .into_iter()
        .map(|(id, reply)| {
            (
                id,
                if matches!(id, PromptId::ContestEndTime | PromptId::GatewayNotAfterTime) {
                    Reply::text("18:00")
                } else {
                    reply
                },
            )
        })
        .collect();
    import_control(&mut prompts, false);
    prompts.extend([
        (PromptId::ServerTlsSource, Reply::Select(2)),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
    ]);
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::PendingSignature { .. }
    ));
    let end = "2029-01-02T10:00:00Z";
    let expiry = "2029-01-03T10:00:00Z";
    let request = fs::read_to_string(fixture.path("/admin/pki/pending/request.toml"))?;
    assert!(
        request.contains(end) && request.contains(expiry),
        "pending parameters must contain UTC timestamps"
    );
    let Some(plan) = script
        .notices
        .iter()
        .find(|notice| matches!(notice, Notice::PlanSummary { .. }))
    else {
        panic!("CSR plan must be displayed")
    };
    let plan = plan.to_string();
    assert!(plan.contains(
        "Contest end: 2029-01-02 18:00 (Asia/Shanghai, UTC+08:00) → 2029-01-02T10:00:00Z"
    ));
    assert!(plan.contains(
        "Gateway expiry: 2029-01-03 18:00 (Asia/Shanghai, UTC+08:00) → 2029-01-03T10:00:00Z"
    ));
    assert!(script.notices.iter().any(|notice| notice.to_string()
        == "Contest end: 2029-01-02 18:00 (Asia/Shanghai, UTC+08:00) → 2029-01-02T10:00:00Z"));

    let signed = sign_pending(&fixture, &ca, &key, false)?;
    let (origin, origin_key) =
        generate::generate_ca("Imported Origin", "Natsume", fixture.host.now)?;
    fixture.host.input("signed.crt", signed.der());
    fixture.host.input("origin.crt", origin.der());
    fixture.host.input("origin.key", origin_key.pkcs8_der());
    fixture.host.offset = time::UtcOffset::from_hms(-8, 0, 0)?;
    fixture.host.timezone.name = Some("America/Los_Angeles".into());
    fixture.host.install_probes(ProbeExit::Passed);
    let mut script = ScriptedInteraction::new(vec![
        (PromptId::SignedCertificatePath, Reply::text("signed.crt")),
        (PromptId::OriginCaSource, Reply::Select(1)),
        (PromptId::OriginCaCertificatePath, Reply::text("origin.crt")),
        (PromptId::OriginCaKeyPath, Reply::text("origin.key")),
        (PromptId::PlanConfirmation, Reply::Confirm(true)),
    ]);
    assert!(matches!(
        fixture.run(&mut script)?,
        WizardOutcome::Installed(_)
    ));
    fixture.assert_installed()?;
    let config = fs::read_to_string(fixture.path(config::CONFIG_PATH))?;
    assert!(config.contains(&format!("contest_end = \"{end}\"")));
    assert!(config.contains(&format!("gateway_not_after = \"{expiry}\"")));
    let Some(plan) = script
        .notices
        .iter()
        .find(|notice| matches!(notice, Notice::PlanSummary { .. }))
    else {
        panic!("resumed plan must be displayed")
    };
    let plan = plan.to_string();
    assert!(
        plan.contains("2029-01-02 02:00 (America/Los_Angeles, UTC-08:00) → 2029-01-02T10:00:00Z")
    );
    assert!(
        plan.contains("2029-01-03 02:00 (America/Los_Angeles, UTC-08:00) → 2029-01-03T10:00:00Z")
    );
    assert!(
        script.date_defaults.is_empty(),
        "CSR continuation must not prompt for dates"
    );
    Ok(())
}

#[test]
fn negative_offset_converts_local_times_across_utc_day_boundary() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.offset = time::UtcOffset::from_hms(-8, 0, 0)?;
    fixture.host.timezone.name = Some("America/Los_Angeles".into());
    let mut prompts: Vec<_> = parameters()
        .into_iter()
        .map(|(id, reply)| {
            (
                id,
                if matches!(id, PromptId::ContestEndTime | PromptId::GatewayNotAfterTime) {
                    Reply::text("23:30")
                } else {
                    reply
                },
            )
        })
        .collect();
    prompts.push((PromptId::ControlCaSource, Reply::Interrupt));
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    for expected in [
        "Contest end: 2029-01-02 23:30 (America/Los_Angeles, UTC-08:00) → 2029-01-03T07:30:00Z",
        "Gateway expiry: 2029-01-03 23:30 (America/Los_Angeles, UTC-08:00) → 2029-01-04T07:30:00Z",
    ] {
        assert!(
            script
                .notices
                .iter()
                .any(|notice| notice.to_string() == expected),
            "missing conversion: {expected}"
        );
    }
    Ok(())
}

#[test]
fn calendar_and_gateway_defaults_use_local_today_and_exactly_twenty_four_hours() -> TestResult {
    let mut fixture = Fixture::new()?;
    fixture.host.now = OffsetDateTime::parse("2029-01-01T20:00:00Z", &Rfc3339)?;
    fixture.host.offset = time::UtcOffset::from_hms(8, 0, 0)?;
    let mut prompts: Vec<_> = parameters()
        .into_iter()
        .map(|(id, reply)| {
            (
                id,
                match id {
                    PromptId::ContestEndDate
                    | PromptId::GatewayNotAfterDate
                    | PromptId::GatewayNotAfterTime => Reply::Default,
                    PromptId::ContestEndTime => Reply::text("18:00"),
                    _ => reply,
                },
            )
        })
        .collect();
    prompts.push((PromptId::ControlCaSource, Reply::Interrupt));
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    let date = OffsetDateTime::parse(END, &Rfc3339)?.date();
    let next = OffsetDateTime::parse(EXPIRY, &Rfc3339)?.date();
    assert_eq!(
        script.date_defaults,
        [
            (PromptId::ContestEndDate, date, Some(date)),
            (PromptId::GatewayNotAfterDate, next, Some(date)),
        ]
    );
    assert!(
        script
            .defaults
            .contains(&(PromptId::GatewayNotAfterTime, "18:00".into()))
    );
    assert!(
        !script
            .defaults
            .iter()
            .any(|(id, _)| *id == PromptId::ContestEndTime),
        "contest time has no default"
    );
    let timestamps: Vec<_> = script
        .notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::SelectedTime { summary, .. } => {
                Some(OffsetDateTime::parse(&summary.utc, &Rfc3339))
            }
            _ => None,
        })
        .collect::<Result<_, _>>()?;
    let mut timestamps = timestamps.into_iter();
    let (Some(end), Some(expiry)) = (timestamps.next(), timestamps.next()) else {
        panic!("both deadline conversions must be displayed")
    };
    assert_eq!(
        end,
        OffsetDateTime::parse("2029-01-02T10:00:00Z", &Rfc3339)?
    );
    assert_eq!(expiry - end, time::Duration::seconds(86400));
    assert!(
        !script
            .notices
            .iter()
            .any(|notice| matches!(notice, Notice::ValidationFailed { .. }))
    );
    Ok(())
}

#[test]
fn invalid_hh_mm_reprompts_the_same_time_field() -> TestResult {
    let mut fixture = Fixture::new()?;
    let invalid = [
        "",
        "8:00",
        "18:0",
        "24:00",
        "23:60",
        "-1:00",
        " 18:00",
        "18:00 ",
        "１８:００",
        "18:00:00",
        "18:00Z",
    ];
    let mut prompts: Vec<_> = parameters().into_iter().take(5).collect();
    prompts.extend(
        invalid
            .iter()
            .map(|time| (PromptId::ContestEndTime, Reply::text(time))),
    );
    prompts.extend(parameters().into_iter().skip(5));
    prompts.push((PromptId::ControlCaSource, Reply::Interrupt));
    let mut script = ScriptedInteraction::new(prompts);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert_eq!(
        script
            .notices
            .iter()
            .filter(|notice| matches!(notice, Notice::ValidationFailed { .. }))
            .count(),
        invalid.len()
    );
    assert_eq!(
        script.date_defaults.len(),
        2,
        "invalid time must not reask dates"
    );
    Ok(())
}

#[test]
fn ambiguous_and_nonexistent_local_times_reprompt_only_the_time() -> TestResult {
    use crate::host::LocalTimeError;
    for field in [PromptId::ContestEndTime, PromptId::GatewayNotAfterTime] {
        for error in [LocalTimeError::Ambiguous, LocalTimeError::Nonexistent] {
            let mut fixture = Fixture::new()?;
            let timestamp = if field == PromptId::ContestEndTime {
                END
            } else {
                EXPIRY
            };
            let local = OffsetDateTime::parse(timestamp, &Rfc3339)?
                .date()
                .with_time(time::Time::from_hms(18, 0, 0)?);
            let expected = error.to_string();
            fixture.host.local_time_errors.insert(local, error);
            let mut prompts = Vec::new();
            for (id, reply) in parameters() {
                if id == field {
                    prompts.push((id, Reply::text("18:00")));
                }
                prompts.push((id, reply));
            }
            prompts.push((PromptId::ControlCaSource, Reply::Interrupt));
            let mut script = ScriptedInteraction::new(prompts);
            assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
            assert_eq!(
                script.date_defaults.len(),
                2,
                "timezone errors must not reask dates"
            );
            assert!(script.notices.iter().any(|notice| matches!(notice, Notice::ValidationFailed { message } if message.contains(&expected))), "missing {expected}");
        }
    }
    Ok(())
}

#[test]
fn escape_from_each_time_returns_to_its_date_and_clears_later_answers() -> TestResult {
    for field in [PromptId::ContestEndTime, PromptId::GatewayNotAfterTime] {
        let mut fixture = Fixture::new()?;
        let mut prompts = Vec::new();
        let date_id = if field == PromptId::ContestEndTime {
            PromptId::ContestEndDate
        } else {
            PromptId::GatewayNotAfterDate
        };
        for (id, reply) in parameters() {
            if id == field {
                prompts.push((id, Reply::Back));
                let timestamp = if field == PromptId::ContestEndTime {
                    EXPIRY
                } else {
                    END
                };
                prompts.push((
                    date_id,
                    Reply::Date(OffsetDateTime::parse(timestamp, &Rfc3339)?.date()),
                ));
                prompts.push((id, Reply::text("00:00")));
                if field == PromptId::ContestEndTime {
                    break;
                }
                prompts.push((
                    PromptId::GatewayNotAfterDate,
                    Reply::Date(OffsetDateTime::parse(EXPIRY, &Rfc3339)?.date()),
                ));
            }
            prompts.push((id, reply));
        }
        if field == PromptId::ContestEndTime {
            prompts.extend([
                (PromptId::GatewayNotAfterDate, Reply::Default),
                (PromptId::GatewayNotAfterTime, Reply::Default),
            ]);
        }
        prompts.push((PromptId::ControlCaSource, Reply::Interrupt));
        let mut script = ScriptedInteraction::new(prompts);
        assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
        let expected = if field == PromptId::ContestEndTime {
            "2029-01-03T00:00:00Z"
        } else {
            END
        };
        assert!(script.notices.iter().any(|notice| matches!(notice, Notice::SelectedTime { field: id, summary } if *id == field && summary.utc == expected)));
    }
    Ok(())
}

#[test]
fn unknown_timezone_requires_preflight_warning_acknowledgement() -> TestResult {
    use crate::preflight::PreflightWarning;
    let mut fixture = Fixture::new()?;
    fixture.host.timezone.name = None;
    let mut script = ScriptedInteraction::new(vec![(
        PromptId::ContinueAfterWarnings,
        Reply::Confirm(false),
    )]);
    assert!(matches!(fixture.run(&mut script)?, WizardOutcome::Aborted));
    assert!(script.notices.iter().any(|notice| matches!(notice, Notice::PreflightWarnings { warnings } if warnings.contains(&PreflightWarning::TimezoneUnknown))));
    assert!(
        script
            .notices
            .iter()
            .any(|notice| notice.to_string().contains("Server timezone: unknown"))
    );
    fixture.assert_no_deployment()?;
    Ok(())
}
