use super::super::Config;
use super::*;
use snafu::Snafu;

#[derive(Debug, Snafu)]
enum TestError {
    Io {
        source: std::io::Error,
    },
    Initialization {
        source: super::super::Error,
    },
    Signal {
        source: tokio::sync::oneshot::error::RecvError,
    },
    Timeout {
        source: tokio::time::error::Elapsed,
    },
    Roster {
        source: crate::roster::Error,
    },
    Fixture,
}

#[test]
fn recovery_cleanup_only_removes_the_expected_directory_and_retains_the_lock() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let parent = std::fs::canonicalize(directory.path()).context(IoSnafu)?;
    let target = parent.join("workspace");
    std::fs::create_dir(&target).context(IoSnafu)?;
    let root = recovery_path(&target).context(InitializationSnafu)?;
    std::fs::create_dir(&root).context(IoSnafu)?;
    std::fs::write(root.join("cache"), b"download").context(IoSnafu)?;
    std::fs::write(target.join("keep"), b"published").context(IoSnafu)?;
    let lock_path = root.with_extension("photo-init.lock");
    let lock = lock_initialization(&lock_path).context(InitializationSnafu)?;
    assert!(remove_recovery_directory(&target, &target).is_err());
    assert!(target.join("keep").exists());
    remove_recovery_directory(&root, &target).context(InitializationSnafu)?;
    assert!(!root.exists());
    assert!(lock_path.is_file());
    assert!(matches!(
        lock_initialization(&lock_path),
        Err(super::super::Error::Busy)
    ));
    drop(lock);
    drop(lock_initialization(&lock_path).context(InitializationSnafu)?);
    assert_eq!(
        std::fs::read(target.join("keep")).context(IoSnafu)?,
        b"published"
    );
    Ok(())
}

#[test]
fn failed_temporary_write_leaves_no_published_file_or_partial_temporary() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let result = write_synced_temporary(directory.path(), |file| {
        file.write_all(b"partial")?;
        Err(std::io::Error::from(std::io::ErrorKind::WriteZero))
    });
    assert!(matches!(result, Err(super::super::Error::Io { .. })));
    assert!(
        std::fs::read_dir(directory.path())
            .context(IoSnafu)?
            .next()
            .is_none()
    );
    Ok(())
}

#[test]
fn atomic_records_preserve_existing_content_and_downloads_repair_partial_cache() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let record = directory.path().join("authorization.json");
    save_new_or_verify(&record, b"original").context(InitializationSnafu)?;
    save_new_or_verify(&record, b"original").context(InitializationSnafu)?;
    assert!(matches!(
        save_new_or_verify(&record, b"changed!"),
        Err(super::super::Error::FileConflict)
    ));
    assert_eq!(std::fs::read(&record).context(IoSnafu)?, b"original");
    let bytes = b"complete server-verified resource";
    let digest = Sha256Digest::of(bytes);
    let cache = directory.path().join(format!("{digest}.png"));
    std::fs::write(&cache, &bytes[..4]).context(IoSnafu)?;
    save_download(directory.path(), &digest, bytes).context(InitializationSnafu)?;
    assert_eq!(std::fs::read(&cache).context(IoSnafu)?, bytes);
    File::options()
        .write(true)
        .open(&cache)
        .context(IoSnafu)?
        .set_len(natsume_photo_protocol::IMAGE_LIMIT as u64 + 1)
        .context(IoSnafu)?;
    save_download(directory.path(), &digest, bytes).context(InitializationSnafu)?;
    assert_eq!(std::fs::read(cache).context(IoSnafu)?, bytes);
    assert_eq!(
        std::fs::read_dir(directory.path())
            .context(IoSnafu)?
            .count(),
        2
    );
    Ok(())
}

#[test]
fn cancelled_workspace_preparation_retains_initialization_ownership_until_io_finishes() -> TestResult
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .context(IoSnafu)?;
    runtime.block_on(async {
        let directory = tempfile::tempdir().context(IoSnafu)?;
        let config = Config {
            server_url: "https://localhost".into(),
            workspace: directory.path().join("workspace"),
            ca_certificate: None,
        };
        let store = Store::open(
            &config.workspace,
            &ServerOrigin::parse(&config.server_url)
                .unwrap_or_else(|error| panic!("valid fixture origin: {error:?}")),
        )
        .context(InitializationSnafu)?;
        let root = store.root().join("prepared");
        let roster = crate::roster::Roster::parse(include_bytes!("../fixtures/roster.json"))
            .context(RosterSnafu)?;
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            let _ = wait.recv();
        });
        started.await.context(SignalSnafu)?;
        let lease = store.lease();
        let prepare_root = root.clone();
        let source = directory.path().to_owned();
        let mut prepare = Box::pin(lease.run(move || {
            Workspace::create_with_logos(prepare_root, roster.teams().to_vec(), &source, Vec::new())
                .context(super::super::WorkspaceSnafu)
        }));
        // Poll the real factory until it has queued its owned blocking operation.
        tokio::select! { biased;
            _ = &mut prepare => return FixtureSnafu.fail(),
            () = std::future::ready(()) => {}
        }
        drop(prepare);
        drop(lease);
        drop(store);
        assert!(matches!(
            Store::open(
                &config.workspace,
                &ServerOrigin::parse(&config.server_url)
                    .unwrap_or_else(|error| panic!("valid fixture origin: {error:?}"))
            ),
            Err(super::super::Error::Busy)
        ));
        drop(release);
        blocker
            .await
            .context(super::super::WorkerSnafu)
            .context(InitializationSnafu)?;
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match Store::open(
                    &config.workspace,
                    &ServerOrigin::parse(&config.server_url)
                        .unwrap_or_else(|error| panic!("valid fixture origin: {error:?}")),
                ) {
                    Ok(store) => {
                        drop(store);
                        break Ok(());
                    }
                    Err(super::super::Error::Busy) => tokio::task::yield_now().await,
                    Err(error) => break Err(error),
                }
            }
        })
        .await
        .context(TimeoutSnafu)?
        .context(InitializationSnafu)?;
        let prepared = Workspace::open(&root)
            .await
            .context(super::super::WorkspaceSnafu)
            .context(InitializationSnafu)?;
        assert_eq!(
            prepared
                .roster()
                .await
                .context(super::super::WorkspaceSnafu)
                .context(InitializationSnafu)?
                .teams()
                .len(),
            2
        );
        Ok(())
    })
}
type TestResult = std::result::Result<(), TestError>;

#[test]
fn a_drive_root_is_an_invalid_target_path_not_a_journal_failure() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let root = directory
        .path()
        .ancestors()
        .last()
        .unwrap_or_else(|| panic!("temporary directory has a drive root"));
    let origin = ServerOrigin::parse("https://localhost")
        .context(super::super::OriginSnafu)
        .context(InitializationSnafu)?;
    assert!(matches!(
        Store::open(root, &origin),
        Err(super::super::Error::Path)
    ));
    Ok(())
}

#[tokio::test]
async fn cancelling_the_caller_keeps_the_lock_until_blocking_io_finishes() -> TestResult {
    cancelled_blocking_work(BlockingBoundary::Files).await?;
    cancelled_blocking_work(BlockingBoundary::Credentials).await
}

enum BlockingBoundary {
    Files,
    Credentials,
}

async fn cancelled_blocking_work(boundary: BlockingBoundary) -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let config = Config {
        server_url: "https://localhost".into(),
        workspace: directory.path().join("workspace"),
        ca_certificate: None,
    };
    let store = Store::open(
        &config.workspace,
        &ServerOrigin::parse(&config.server_url)
            .unwrap_or_else(|error| panic!("valid fixture origin: {error:?}")),
    )
    .context(InitializationSnafu)?;
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, waiting) = std::sync::mpsc::channel::<()>();
    let task = tokio::spawn(async move {
        let operation = move || {
            let _ = started.send(());
            // Dropping the sender deliberately releases this fixture's worker.
            let _ = waiting.recv();
            Ok(())
        };
        match boundary {
            BlockingBoundary::Files => store.lease().run(operation).await,
            BlockingBoundary::Credentials => {
                let lease = store.lease();
                drop(store);
                lease.run(operation).await
            }
        }
    });
    entered.await.context(SignalSnafu)?;
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    assert!(matches!(
        Store::open(
            &config.workspace,
            &ServerOrigin::parse(&config.server_url)
                .unwrap_or_else(|error| panic!("valid fixture origin: {error:?}"))
        ),
        Err(super::super::Error::Busy)
    ));
    drop(release);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match Store::open(
                &config.workspace,
                &ServerOrigin::parse(&config.server_url)
                    .unwrap_or_else(|error| panic!("valid fixture origin: {error:?}")),
            ) {
                Ok(store) => {
                    drop(store);
                    return Ok(());
                }
                Err(super::super::Error::Busy) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .context(TimeoutSnafu)?
    .context(InitializationSnafu)
}
