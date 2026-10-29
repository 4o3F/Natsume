mod fake_host;
mod scenarios;
mod script;

use super::*;
use crate::{
    config,
    deployment::{file::Ownership, plan},
    pki::{generate, import},
};
use fake_host::FakeHost;
use script::{Reply, ScriptedInteraction};
use snafu::Snafu;
use std::{
    fs, io,
    net::Ipv4Addr,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::Path,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Snafu)]
enum TestError {
    #[snafu(context(false), display("fixture I/O failed"))]
    Io { source: io::Error },
    #[snafu(context(false), display("wizard scenario failed"))]
    Wizard { source: WizardError },
    #[snafu(context(false), display("fixture generation failed"))]
    Generate { source: GenerateError },
    #[snafu(context(false), display("fixture import failed"))]
    Import { source: ImportError },
    #[snafu(context(false), display("fixture pending operation failed"))]
    Pending { source: PendingError },
    #[snafu(context(false), display("fixture parameters failed"))]
    Config { source: ConfigRenderError },
    #[snafu(context(false), display("fixture time parsing failed"))]
    Time { source: time::error::Parse },
    #[snafu(context(false), display("fixture certificate construction failed"))]
    Certificate { source: rcgen::Error },
    #[snafu(context(false), display("fixture key encoding failed"))]
    Der { source: pkcs8::der::Error },
    #[snafu(context(false), display("fixture PEM encoding failed"))]
    Pem { source: pkcs8::der::pem::Error },
    #[snafu(context(false), display("fixture detection failed"))]
    Plan { source: plan::PlanError },
}

type TestResult = Result<(), TestError>;
const PASSPHRASE: &str = "wizard-secret-canary-never-log-this";
const END: &str = "2029-01-02T00:00:00Z";
const EXPIRY: &str = "2029-01-03T00:00:00Z";

struct Fixture {
    root: tempfile::TempDir,
    context: WizardContext,
    host: FakeHost,
}

impl Fixture {
    fn new() -> Result<Self, TestError> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("etc"))?;
        fs::create_dir_all(root.path().join("var/lib/natsume-server"))?;
        fs::set_permissions(
            root.path().join("var/lib/natsume-server"),
            fs::Permissions::from_mode(0o750),
        )?;
        let owner = Ownership {
            uid: rustix::process::geteuid().as_raw(),
            gid: rustix::process::getegid().as_raw(),
        };
        let context = WizardContext {
            target_root: root.path().into(),
            pki_dir: "/admin/pki".into(),
            root_owner: owner,
        };
        let host = FakeHost::new(OffsetDateTime::parse("2029-01-01T00:00:00Z", &Rfc3339)?);
        Ok(Self {
            root,
            context,
            host,
        })
    }
    fn path(&self, path: &str) -> PathBuf {
        self.root.path().join(path.trim_start_matches('/'))
    }
    fn run(&mut self, script: &mut ScriptedInteraction) -> Result<WizardOutcome, WizardError> {
        let outcome = run(script, &mut self.host, &self.context);
        script.assert_finished();
        self.host.assert_finished();
        outcome
    }
    fn system_files(&self) -> Result<Vec<PathBuf>, plan::PlanError> {
        plan::detect_existing_deployment(self.root.path())
    }
    fn assert_no_deployment(&self) -> TestResult {
        assert!(
            self.system_files()?.is_empty(),
            "cancelled/pending run wrote deployment files"
        );
        Ok(())
    }
    fn assert_installed(&self) -> TestResult {
        let files = self.system_files()?;
        assert_eq!(
            files.len(),
            7,
            "installation must publish exactly seven deployment files"
        );
        for path in files {
            let metadata = fs::metadata(&path)?;
            let private = path.starts_with(self.path(config::KEYS_DIRECTORY));
            assert_eq!(
                metadata.mode() & 0o7777,
                if private { 0o600 } else { 0o644 },
                "file mode {path:?}"
            );
            assert_eq!(
                (metadata.uid(), metadata.gid()),
                (self.context.root_owner.uid, self.context.root_owner.gid)
            );
        }
        for (directory, mode) in [
            (config::CONFIG_DIRECTORY, 0o755),
            (config::TRUST_PARENT_DIRECTORY, 0o755),
            (config::TRUST_DIRECTORY, 0o755),
            (config::KEYS_DIRECTORY, 0o700),
        ] {
            assert_eq!(
                fs::metadata(self.path(directory))?.mode() & 0o7777,
                mode,
                "directory mode {directory}"
            );
        }
        let mut expected: Vec<_> = [
            config::CONFIG_PATH,
            config::CONTROL_CA_TRUST,
            config::LOCAL_ORIGIN_TRUST,
        ]
        .map(|path| self.path(path))
        .into();
        expected.sort();
        assert_eq!(
            files_under(&self.path("/etc"))?,
            expected,
            "unexpected system configuration files"
        );
        let mut expected: Vec<_> = [
            config::ORIGIN_CA_DER,
            config::ORIGIN_CA_KEY,
            config::SERVER_TLS_LEAF,
            config::SERVER_TLS_KEY,
        ]
        .map(|path| self.path(path))
        .into();
        if self.path(config::ROOT_KEY).exists() {
            expected.push(self.path(config::ROOT_KEY));
        }
        expected.sort();
        assert_eq!(
            files_under(&self.path(config::KEYS_DIRECTORY))?,
            expected,
            "unexpected service identity files"
        );
        Ok(())
    }
}

fn parameters() -> Vec<(crate::interaction::PromptId, Reply)> {
    use crate::interaction::PromptId;
    vec![
        (PromptId::ServerIps, Reply::text("127.0.0.1")),
        (PromptId::HttpsPort, Reply::text("8443")),
        (
            PromptId::GatewayHostname,
            Reply::text("gateway.contest.example"),
        ),
        (
            PromptId::DomjudgeOrigin,
            Reply::text("https://judge.contest.example"),
        ),
        (PromptId::ContestEnd, Reply::text(END)),
        (PromptId::GatewayNotAfter, Reply::text(EXPIRY)),
    ]
}

fn generated_prefix() -> Vec<(crate::interaction::PromptId, Reply)> {
    use crate::interaction::PromptId;
    let mut prompts = parameters();
    prompts.extend([
        (PromptId::ControlCaSource, Reply::Select(0)),
        (PromptId::ServerTlsSource, Reply::Select(0)),
        (PromptId::OriginCaSource, Reply::Select(0)),
    ]);
    for role in [CaRole::Control, CaRole::Origin] {
        prompts.push((
            PromptId::NewCaPassphrase { role },
            Reply::secret(PASSPHRASE),
        ));
    }
    prompts.push((PromptId::PlanConfirmation, Reply::Confirm(true)));
    prompts
}

fn retention(save: bool) -> Vec<(crate::interaction::PromptId, Reply)> {
    use crate::interaction::PromptId;
    let mut prompts = Vec::new();
    for role in [CaRole::Control, CaRole::Origin] {
        prompts.push((PromptId::KeyPersistence { role }, Reply::Confirm(save)));
        if !save {
            prompts.push((
                PromptId::KeyRetentionConfirmation { role },
                Reply::text("key saved"),
            ));
        }
    }
    prompts
}

fn generated_script(save: bool) -> ScriptedInteraction {
    let mut prompts = generated_prefix();
    prompts.extend(retention(save));
    ScriptedInteraction::new(prompts)
}

fn files_under(path: &Path) -> Result<Vec<PathBuf>, io::Error> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            paths.extend(files_under(&entry.path())?);
        } else {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn assert_public_handoff(summary: &InstallationSummary) -> TestResult {
    let Ok(report) = &summary.handoff else {
        panic!("public handoff must be published")
    };
    let files = files_under(&report.directory)?;
    assert_eq!(
        files.len(),
        4,
        "handoff must contain config, two roots, and checksums"
    );
    for path in files {
        assert_eq!(fs::metadata(path)?.mode() & 0o7777, 0o644);
    }
    Ok(())
}
