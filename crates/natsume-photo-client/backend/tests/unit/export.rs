use super::*;
use crate::{
    initialization::{ClientId, SecretSlot, ServerOrigin},
    photo::{ConfirmationId, PhotoDimensions, PhotoFormat},
    roster::Roster,
    workspace::{ConfirmedPhoto, FinalDraft},
};
use natsume_photo_protocol::{DatasetId, InitializationId, PhotoInitialization, SourceId};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

#[derive(Debug, Snafu)]
enum TestError {
    Io {
        source: std::io::Error,
    },
    Export {
        source: ExportError,
    },
    Workspace {
        source: crate::workspace::Error,
    },
    Final {
        source: crate::workspace::FinalPhotoError,
    },
    Roster {
        source: crate::roster::Error,
    },
    Json {
        source: serde_json::Error,
    },
    Image {
        source: image::ImageError,
    },
    Value {
        source: natsume_photo_protocol::ValueError,
    },
    Origin {
        source: initialization::OriginError,
    },
    Zip {
        source: zip::result::ZipError,
    },
    Parse {
        source: std::num::ParseIntError,
    },
    Worker {
        source: tokio::task::JoinError,
    },
    Channel {
        source: std::sync::mpsc::RecvTimeoutError,
    },
    Timeout {
        source: tokio::time::error::Elapsed,
    },
    Fixture,
}
type TestResult<T = ()> = std::result::Result<T, TestError>;
#[derive(Clone)]
struct MemorySecrets(Arc<Mutex<Option<Zeroizing<String>>>>);
impl Secrets for MemorySecrets {
    fn load(
        &self,
        _: &ServerOrigin,
        _: &SecretSlot,
    ) -> initialization::CredentialResult<Option<Zeroizing<String>>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|_| panic!("fixture mutex poisoned"))
            .clone())
    }
    fn save(
        &self,
        _: &ServerOrigin,
        _: &SecretSlot,
        _: &str,
    ) -> initialization::CredentialResult<()> {
        panic!("offline export must never replace a signing key")
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    workspace: Workspace,
    secrets: MemorySecrets,
    identity: doctor::Authorization,
}

#[derive(Clone)]
struct GatedSecrets {
    inner: MemorySecrets,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    entered: std::sync::mpsc::Sender<()>,
    release: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}
impl Secrets for GatedSecrets {
    fn load(
        &self,
        origin: &ServerOrigin,
        slot: &SecretSlot,
    ) -> initialization::CredentialResult<Option<Zeroizing<String>>> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            let _ = self.entered.send(());
            self.release
                .lock()
                .unwrap_or_else(|_| panic!("fixture gate poisoned"))
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|error| panic!("fixture gate failed: {error:?}"));
        }
        self.inner.load(origin, slot)
    }
    fn save(
        &self,
        _: &ServerOrigin,
        _: &SecretSlot,
        _: &str,
    ) -> initialization::CredentialResult<()> {
        panic!("export cannot save credentials")
    }
}

#[tokio::test]
async fn cancelled_waiter_does_not_release_workspace_or_cancel_an_accepted_export() -> TestResult {
    let f = fixture().await?;
    save(&f, 0, 30).await?;
    let target = output(&f);
    let observed = target.clone();
    let root = f.workspace.root().to_owned();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let secrets = GatedSecrets {
        inner: f.secrets,
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        entered,
        release: Arc::new(Mutex::new(gate)),
    };
    let workspace = f.workspace;
    let task = tokio::spawn(async move { export_results(&workspace, &target, &secrets).await });
    tokio::task::spawn_blocking(move || ready.recv_timeout(std::time::Duration::from_secs(10)))
        .await
        .context(WorkerSnafu)?
        .context(ChannelSnafu)?;
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    assert!(matches!(
        Workspace::open(&root).await,
        Err(crate::workspace::Error::Storage {
            source: crate::workspace::StorageError::Busy
        })
    ));
    ensure!(release.send(()).is_ok(), FixtureSnafu);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match Workspace::open(&root).await {
                Ok(workspace) => {
                    drop(workspace);
                    break;
                }
                Err(crate::workspace::Error::Storage {
                    source: crate::workspace::StorageError::Busy,
                }) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                Err(source) => return Err(TestError::Workspace { source }),
            }
        }
        Ok(())
    })
    .await
    .context(TimeoutSnafu)??;
    assert!(observed.is_file());
    Ok(())
}
async fn fixture() -> TestResult<Fixture> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../fixtures/roster.json")).context(RosterSnafu)?;
    let workspace = crate::workspace::fixture::create(&directory.path().join("workspace"), &roster)
        .await
        .context(WorkspaceSnafu)?;
    let manifest = PhotoInitialization {
        format_version: FormatVersion::V1,
        source_id: SourceId::generate(),
        dataset_id: DatasetId::generate(),
        initialization_id: InitializationId::generate(),
        teams: roster.teams().to_vec(),
        school_logos: Vec::new(),
    };
    let bytes = serde_json::to_vec(&manifest).context(JsonSnafu)?;
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let identity = doctor::Authorization {
        server_url: ServerOrigin::parse("https://offline.example.test:8443")
            .context(OriginSnafu)?,
        client_id: ClientId::generate(),
        signing_public_key: hex::encode(key.verifying_key().to_bytes())
            .parse()
            .context(ValueSnafu)?,
        source_id: manifest.source_id,
        dataset_id: manifest.dataset_id,
        initialization_id: manifest.initialization_id,
        manifest_sha256: Sha256Digest::of(&bytes),
    };
    fs::write(workspace.root().join("initialization.json"), bytes).context(IoSnafu)?;
    fs::write(
        workspace.root().join("authorization.json"),
        serde_json::to_vec(&identity).context(JsonSnafu)?,
    )
    .context(IoSnafu)?;
    fs::create_dir(workspace.root().join("captures")).context(IoSnafu)?;
    Ok(Fixture {
        directory,
        workspace,
        secrets: MemorySecrets(Arc::new(Mutex::new(Some(Zeroizing::new(hex::encode(
            [7; 32],
        )))))),
        identity,
    })
}
async fn save(f: &Fixture, team: usize, color: u8) -> TestResult<ConfirmedPhoto> {
    let path = f
        .workspace
        .root()
        .join("captures")
        .join(format!("{}.png", uuid::Uuid::now_v7()));
    image::RgbImage::from_pixel(8, 6, image::Rgb([color; 3]))
        .save(&path)
        .context(ImageSnafu)?;
    let bytes = fs::read(&path).context(IoSnafu)?;
    let roster = f.workspace.roster().await.context(WorkspaceSnafu)?;
    let account = roster
        .teams()
        .get(team)
        .ok_or_else(|| FixtureSnafu.build())?;
    f.workspace
        .final_store()
        .save(&FinalDraft {
            account_id: account.account_id.clone(),
            confirmation_id: ConfirmationId::generate(),
            source: path,
            sha256: Sha256Digest::of(&bytes).to_string(),
            bytes: bytes.len() as u64,
            dimensions: PhotoDimensions {
                width: 8,
                height: 6,
                format: PhotoFormat::Png,
            },
        })
        .context(FinalSnafu)
}
fn output(f: &Fixture) -> PathBuf {
    f.directory.path().join("photos.zip")
}

#[tokio::test]
async fn export_is_independently_readable_deduplicates_and_retries_across_reopen() -> TestResult {
    let f = fixture().await?;
    let first = save(&f, 0, 30).await?;
    let second = save(&f, 1, 30).await?;
    assert_eq!(first.path(), second.path());
    let result = export_results(&f.workspace, &output(&f), &f.secrets)
        .await
        .context(ExportSnafu)?;
    let mut zip =
        zip::ZipArchive::new(File::open(output(&f)).context(IoSnafu)?).context(ZipSnafu)?;
    assert_eq!(zip.len(), 2);
    let mut json = Vec::new();
    zip.by_name("manifest.json")
        .context(ZipSnafu)?
        .read_to_end(&mut json)
        .context(IoSnafu)?;
    let context = ResultContext {
        source_id: f.identity.source_id.clone(),
        dataset_id: f.identity.dataset_id.clone(),
        initialization_id: f.identity.initialization_id.clone(),
        initialization_sha256: f.identity.manifest_sha256.clone(),
    };
    let signed = SignedResults::parse_and_verify(&json, &context, &f.identity.signing_public_key)
        .context(super::ContractSnafu)
        .context(ExportSnafu)?;
    assert_eq!(signed.manifest.results.len(), 2);
    assert_eq!(&signed.manifest.results[0].result_id, first.result_id());
    let mut bytes = Vec::new();
    zip.by_name(&signed.manifest.results[0].image_entry())
        .context(ZipSnafu)?
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    assert_eq!(Sha256Digest::of(&bytes).as_str(), first.sha256());
    drop(zip);
    assert_eq!(
        export_results(&f.workspace, &output(&f), &f.secrets)
            .await
            .context(ExportSnafu)?,
        result
    );
    let root = f.workspace.root().to_owned();
    let target = output(&f);
    fs::remove_file(&target).context(IoSnafu)?;
    drop(f.workspace);
    let reopened = Workspace::open(&root).await.context(WorkspaceSnafu)?;
    assert_eq!(
        export_results(&reopened, &target, &f.secrets)
            .await
            .context(ExportSnafu)?,
        result
    );
    Ok(())
}

#[tokio::test]
async fn missing_or_replaced_key_empty_results_and_invalid_targets_cannot_export() -> TestResult {
    let f = fixture().await?;
    assert!(matches!(
        export_results(&f.workspace, &output(&f), &f.secrets).await,
        Err(ExportError::Contract { .. })
    ));
    save(&f, 0, 30).await?;
    *f.secrets
        .0
        .lock()
        .unwrap_or_else(|_| panic!("fixture mutex poisoned")) = None;
    assert!(matches!(
        export_results(&f.workspace, &output(&f), &f.secrets).await,
        Err(ExportError::Authorization {
            source: initialization::Error::MissingSecret
        })
    ));
    *f.secrets
        .0
        .lock()
        .unwrap_or_else(|_| panic!("fixture mutex poisoned")) =
        Some(Zeroizing::new(hex::encode([9; 32])));
    assert!(matches!(
        export_results(&f.workspace, &output(&f), &f.secrets).await,
        Err(ExportError::Authorization {
            source: initialization::Error::Authorization
        })
    ));
    assert!(!output(&f).exists());
    assert!(target_path(f.workspace.root(), &f.workspace.root().join("inside.zip")).is_err());
    assert!(target_path(f.workspace.root(), Path::new("relative.zip")).is_err());
    assert!(
        target_path(
            f.workspace.root(),
            Path::new(r"\\offline.example.test\share\photos.zip")
        )
        .is_err()
    );
    assert!(
        target_path(
            f.workspace.root(),
            &f.directory.path().join("file.zip:stream")
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn target_conflict_and_completion_failure_never_claim_success_and_retry_keeps_identity()
-> TestResult {
    let f = fixture().await?;
    save(&f, 0, 30).await?;
    fs::write(output(&f), b"user file").context(IoSnafu)?;
    assert!(matches!(
        export_results(&f.workspace, &output(&f), &f.secrets).await,
        Err(ExportError::Conflict)
    ));
    assert_eq!(fs::read(output(&f)).context(IoSnafu)?, b"user file");
    let record = fs::read_dir(f.workspace.root().join("exports"))
        .context(IoSnafu)?
        .next()
        .ok_or_else(|| FixtureSnafu.build())?
        .context(IoSnafu)?
        .path();
    let prepared: SignedResults =
        serde_json::from_slice(&fs::read(&record).context(IoSnafu)?).context(JsonSnafu)?;
    let blocked = record.with_extension("complete.json");
    fs::create_dir(&blocked).context(IoSnafu)?;
    fs::remove_file(output(&f)).context(IoSnafu)?;
    assert!(
        export_results(&f.workspace, &output(&f), &f.secrets)
            .await
            .is_err()
    );
    assert!(output(&f).is_file());
    fs::remove_dir(&blocked).context(IoSnafu)?;
    let result = export_results(&f.workspace, &output(&f), &f.secrets)
        .await
        .context(ExportSnafu)?;
    assert_eq!(result.manifest_id, prepared.manifest.manifest_id);
    assert!(blocked.is_file());
    Ok(())
}

#[tokio::test]
async fn pinned_export_keeps_old_bytes_during_retake_and_releases_cleanup_afterward() -> TestResult
{
    let f = fixture().await?;
    let old = save(&f, 0, 30).await?;
    let snapshot = f
        .workspace
        .export_snapshot()
        .await
        .context(WorkspaceSnafu)?;
    let new = save(&f, 0, 80).await?;
    let (_, failures) = f
        .workspace
        .final_store()
        .remove_unused()
        .context(FinalSnafu)?;
    assert_eq!(failures.len(), 1);
    let result = export_blocking(
        f.workspace.root(),
        &output(&f),
        &f.identity,
        &f.secrets,
        snapshot,
    )
    .context(ExportSnafu)?;
    assert_eq!(result.result_count, 1);
    assert_ne!(old.path(), new.path());
    let (_, failures) = f
        .workspace
        .final_store()
        .remove_unused()
        .context(FinalSnafu)?;
    assert!(failures.is_empty());
    assert!(!f.workspace.root().join(old.path()).exists());
    assert!(f.workspace.root().join(new.path()).is_file());
    Ok(())
}

#[tokio::test]
async fn changed_authorization_or_corrupt_journal_is_rejected_without_overwrite() -> TestResult {
    let f = fixture().await?;
    save(&f, 0, 30).await?;
    export_results(&f.workspace, &output(&f), &f.secrets)
        .await
        .context(ExportSnafu)?;
    let record = fs::read_dir(f.workspace.root().join("exports"))
        .context(IoSnafu)?
        .filter_map(std::result::Result::ok)
        .find(|e| !e.file_name().to_string_lossy().contains("complete"))
        .ok_or_else(|| FixtureSnafu.build())?
        .path();
    fs::write(&record, b"bad record").context(IoSnafu)?;
    assert!(
        export_results(&f.workspace, &output(&f), &f.secrets)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&record).context(IoSnafu)?, b"bad record");
    fs::write(f.workspace.root().join("initialization.json"), b"changed").context(IoSnafu)?;
    assert!(matches!(
        export_results(&f.workspace, &output(&f), &f.secrets).await,
        Err(ExportError::Authorization { .. })
    ));
    Ok(())
}

struct BrokenWriter {
    bytes: std::io::Cursor<Vec<u8>>,
    central: bool,
}
impl Write for BrokenWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !self.central || bytes.starts_with(b"PK\x01\x02") {
            return Err(std::io::Error::other("injected write failure"));
        }
        self.bytes.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for BrokenWriter {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        self.bytes.seek(position)
    }
}
#[tokio::test]
async fn image_mismatch_write_and_zip_finalization_failures_are_propagated() -> TestResult {
    let f = fixture().await?;
    save(&f, 0, 30).await?;
    for central in [false, true] {
        let mut snapshot = f
            .workspace
            .export_snapshot()
            .await
            .context(WorkspaceSnafu)?;
        let writer = BrokenWriter {
            bytes: std::io::Cursor::new(Vec::new()),
            central,
        };
        assert!(archive::write(writer, b"{}", &mut snapshot.images).is_err());
    }
    let mut snapshot = f
        .workspace
        .export_snapshot()
        .await
        .context(WorkspaceSnafu)?;
    snapshot.images[0].entry.image_sha256 = Sha256Digest::of(b"wrong");
    assert!(matches!(
        archive::write(
            std::io::Cursor::new(Vec::new()),
            b"{}",
            &mut snapshot.images
        ),
        Err(ExportError::Integrity)
    ));
    assert!(!output(&f).exists());
    Ok(())
}

#[tokio::test]
#[ignore = "large streaming/RSS probe; run explicitly with NATSUME_PHOTO_STRESS_IMAGES"]
async fn streaming_export_stress() -> TestResult {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let mut f = fixture().await?;
    let count = std::env::var("NATSUME_PHOTO_STRESS_IMAGES")
        .unwrap_or_else(|_| "8".into())
        .parse::<usize>()
        .context(ParseSnafu)?;
    let roster = f.workspace.roster().await.context(WorkspaceSnafu)?;
    let base = roster.teams()[0].clone();
    drop(f.workspace);
    // The stress fixture creates a fresh workspace with enough independent test teams.
    let teams = (0..count)
        .map(|i| {
            let mut t = base.clone();
            t.account_id = format!("team-{i:04}");
            t.account = t.account_id.clone();
            t
        })
        .collect();
    f.workspace = Workspace::create_with_logos(
        f.directory.path().join("stress"),
        teams,
        f.directory.path(),
        Vec::new(),
    )
    .context(WorkspaceSnafu)?;
    let manifest = PhotoInitialization {
        format_version: FormatVersion::V1,
        source_id: f.identity.source_id.clone(),
        dataset_id: f.identity.dataset_id.clone(),
        initialization_id: f.identity.initialization_id.clone(),
        teams: f
            .workspace
            .roster()
            .await
            .context(WorkspaceSnafu)?
            .teams()
            .to_vec(),
        school_logos: Vec::new(),
    };
    let bytes = serde_json::to_vec(&manifest).context(JsonSnafu)?;
    f.identity.manifest_sha256 = Sha256Digest::of(&bytes);
    fs::write(f.workspace.root().join("initialization.json"), bytes).context(IoSnafu)?;
    fs::write(
        f.workspace.root().join("authorization.json"),
        serde_json::to_vec(&f.identity).context(JsonSnafu)?,
    )
    .context(IoSnafu)?;
    fs::create_dir(f.workspace.root().join("captures")).context(IoSnafu)?;
    for i in 0..count {
        let mut noise = vec![0_u8; 2048 * 2048 * 3];
        let mut state = i as u64 + 1;
        for byte in &mut noise {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state.to_le_bytes()[0];
        }
        let image =
            image::RgbImage::from_raw(2048, 2048, noise).ok_or_else(|| FixtureSnafu.build())?;
        let path = f.workspace.root().join(format!("captures/{i}.png"));
        image.save(&path).context(ImageSnafu)?;
        drop(image);
        let bytes = fs::read(&path).context(IoSnafu)?;
        f.workspace
            .final_store()
            .save(&FinalDraft {
                account_id: format!("team-{i:04}"),
                confirmation_id: ConfirmationId::generate(),
                source: path,
                sha256: Sha256Digest::of(&bytes).to_string(),
                bytes: bytes.len() as u64,
                dimensions: PhotoDimensions {
                    width: 2048,
                    height: 2048,
                    format: PhotoFormat::Png,
                },
            })
            .context(FinalSnafu)?;
    }
    let result = export_results(&f.workspace, &output(&f), &f.secrets)
        .await
        .context(ExportSnafu)?;
    assert_eq!(result.result_count, count);
    assert!(result.bytes > count as u64 * 12 * 1024 * 1024);
    Ok(())
}
