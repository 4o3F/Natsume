use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use super::*;
use crate::component::contest::logo_io::{DecodedLogo, read_logo};

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><rect width="4" height="3" fill="#f00"/></svg>"##;

fn checked<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    value.unwrap_or_else(|error| panic!("test fixture: {error:?}"))
}

fn school(name: &str) -> OrganizationDetails {
    OrganizationDetails {
        organization_id: 1,
        name_zh: name.to_owned(),
        name_en: "English".to_owned(),
        country: "CHN".to_owned(),
    }
}

async fn response(
    cache: &Arc<LogoHttpCache>,
    directory: &Path,
    organization: OrganizationDetails,
    etag: Option<&str>,
) -> Result<LogoResponse, ExportError> {
    cache
        .response(
            directory.to_owned(),
            organization,
            etag.into_iter().map(str::to_owned).collect(),
            Arc::new(Semaphore::new(4)),
        )
        .await
}

#[tokio::test]
async fn raster_and_svg_keep_existing_etags_and_hits_skip_reader_and_image_permit() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("School.png");
    for svg in [false, true] {
        if svg {
            checked(fs::write(&path, SVG));
        } else {
            checked(image::DynamicImage::new_rgb8(7, 5).save(&path));
        }
        let old = checked(read_logo(&path).and_then(DecodedLogo::into_http));
        let expected = format!("\"{}\"", hex::encode(Sha256::digest(&old.bytes)));
        let cache = Arc::new(LogoHttpCache::default());
        let warm = checked(response(&cache, root.path(), school("School"), None).await);
        assert_eq!(warm.etag, expected);
        assert_eq!(warm.image.map(|image| image.bytes), Some(old.bytes));
        let image_work = Arc::new(Semaphore::new(4));
        let _held = checked(image_work.clone().acquire_many_owned(4).await);
        for condition in [
            expected.clone(),
            format!("\"old\", W/{expected}"),
            "*".to_owned(),
        ] {
            let hit = checked(checked(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    cache.response_with(
                        root.path().to_owned(),
                        school("School"),
                        vec![condition],
                        image_work.clone(),
                        |_| panic!("304 must not read, decode, rasterize or hash image content"),
                    ),
                )
                .await,
            ));
            assert!(hit.image.is_none());
            assert_eq!(hit.etag, expected);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cold_conditional_requests_compute_once() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("School.svg");
    checked(fs::write(&path, SVG));
    let bytes = checked(checked(read_logo(&path)).into_http()).bytes;
    let etag = format!("\"{}\"", hex::encode(Sha256::digest(bytes)));
    let cache = Arc::new(LogoHttpCache::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    let permits = Arc::new(Semaphore::new(4));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let (cache, calls, barrier, permits, etag, directory) = (
            cache.clone(),
            calls.clone(),
            barrier.clone(),
            permits.clone(),
            etag.clone(),
            root.path().to_owned(),
        );
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let response = checked(
                cache
                    .response_with(
                        directory,
                        school("School"),
                        vec![etag],
                        permits,
                        move |source| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(20));
                            source.http_image()
                        },
                    )
                    .await,
            );
            assert!(response.image.is_none());
        }));
    }
    for task in tasks {
        checked(task.await);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replacements_deletions_renames_additions_and_directory_changes_invalidate() {
    let root = checked(tempfile::tempdir());
    let directory = root.path().join("logos");
    checked(fs::create_dir(&directory));
    let path = directory.join("School.svg");
    checked(fs::write(&path, SVG));
    let cache = Arc::new(LogoHttpCache::default());
    let first = checked(response(&cache, &directory, school("School"), None).await);
    let replacement = root.path().join("replacement");
    checked(fs::write(&replacement, SVG.replace("#f00", "#00f")));
    checked(fs::rename(replacement, &path));
    let second = checked(response(&cache, &directory, school("School"), Some(&first.etag)).await);
    assert!(second.image.is_some());
    assert_ne!(first.etag, second.etag);
    checked(fs::rename(&path, directory.join("English.svg")));
    let renamed = checked(response(&cache, &directory, school("School"), Some(&second.etag)).await);
    assert!(renamed.image.is_none());
    checked(fs::write(&path, SVG));
    assert!(matches!(
        response(&cache, &directory, school("School"), Some(&second.etag)).await,
        Err(ExportError::LogoUnavailable)
    ));
    checked(fs::remove_file(directory.join("English.svg")));
    let third = checked(response(&cache, &directory, school("School"), Some(&second.etag)).await);
    assert_eq!(third.etag, first.etag);
    checked(fs::remove_file(&path));
    assert!(matches!(
        response(&cache, &directory, school("School"), Some(&third.etag)).await,
        Err(ExportError::LogoUnavailable)
    ));
    checked(fs::write(&path, SVG));
    checked(response(&cache, &directory, school("School"), None).await);
    checked(fs::rename(&directory, root.path().join("old")));
    checked(fs::create_dir(&directory));
    checked(fs::write(&path, SVG.replace("#f00", "#0f0")));
    let replaced = checked(response(&cache, &directory, school("School"), Some(&first.etag)).await);
    assert!(replaced.image.is_some());
    assert_ne!(replaced.etag, first.etag);
    checked(
        response(
            &cache,
            &root.path().join("old"),
            school("School"),
            Some(&first.etag),
        )
        .await,
    );
    let renamed_school = OrganizationDetails {
        name_en: String::new(),
        ..school("Other")
    };
    assert!(matches!(
        response(
            &cache,
            &directory,
            renamed_school.clone(),
            Some(&replaced.etag)
        )
        .await,
        Err(ExportError::LogoUnavailable)
    ));
    checked(fs::rename(&path, directory.join("Other.svg")));
    assert!(
        checked(response(&cache, &directory, renamed_school, Some(&replaced.etag)).await)
            .image
            .is_none()
    );
}

#[tokio::test]
async fn changes_during_read_are_not_retained_and_symlinks_are_rejected() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("School.svg");
    checked(fs::write(&path, SVG));
    let cache = Arc::new(LogoHttpCache::default());
    let changed = path.clone();
    checked(
        cache
            .response_with(
                root.path().to_owned(),
                school("School"),
                Vec::new(),
                Arc::new(Semaphore::new(4)),
                move |source| {
                    let image = source.http_image()?;
                    checked(fs::write(changed, "corrupt"));
                    Ok(image)
                },
            )
            .await,
    );
    assert!(
        response(&cache, root.path(), school("School"), Some("*"))
            .await
            .is_err()
    );
    checked(fs::remove_file(&path));
    checked(fs::write(root.path().join("target"), SVG));
    checked(std::os::unix::fs::symlink(
        root.path().join("target"),
        &path,
    ));
    assert!(
        matches!(response(&cache, root.path(), school("School"), Some("*")).await, Err(ExportError::Logo { source, .. }) if matches!(*source, LogoError::NotRegularFile))
    );
}

#[tokio::test]
async fn caches_are_bounded_and_evicted_paths_are_recomputed() {
    let root = checked(tempfile::tempdir());
    for index in 0..1100 {
        checked(fs::write(
            root.path().join(format!("School{index}.svg")),
            SVG,
        ));
    }
    let cache = Arc::new(LogoHttpCache::default());
    for index in 0..1000 {
        checked(response(&cache, root.path(), school(&format!("School{index}")), None).await);
    }
    checked(response(&cache, root.path(), school("School0"), None).await);
    for index in 1000..1100 {
        checked(response(&cache, root.path(), school(&format!("School{index}")), None).await);
    }
    {
        let resolutions = cache.resolutions.lock().await;
        assert_eq!(resolutions.entries.len(), MAX_RESOLUTIONS);
        assert!(
            resolutions
                .entries
                .contains_key(&("School0".to_owned(), "English".to_owned()))
        );
        assert!(
            !resolutions
                .entries
                .contains_key(&("School1".to_owned(), "English".to_owned()))
        );
    }
    let mut retained = std::collections::HashSet::new();
    for shard in &cache.etags {
        let entries = shard.entries.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(entries.len() <= ENTRIES_PER_SHARD);
        retained.extend(entries.keys().cloned());
    }
    assert!(retained.len() <= SHARDS * ENTRIES_PER_SHARD);
    assert!(retained.contains(&root.path().join("School0.svg")));
    let evicted = (0..1100)
        .map(|index| format!("School{index}"))
        .find(|name| !retained.contains(&root.path().join(format!("{name}.svg"))))
        .unwrap_or_else(|| panic!("capacity must evict an old path"));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    checked(
        cache
            .response_with(
                root.path().to_owned(),
                school(&evicted),
                vec!["*".to_owned()],
                Arc::new(Semaphore::new(4)),
                move |source| {
                    count.fetch_add(1, Ordering::SeqCst);
                    source.http_image()
                },
            )
            .await,
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_first_request_retains_single_flight_and_publishes_digest() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("School.svg");
    checked(fs::write(&path, SVG));
    let expected = checked(checked(read_logo(&path)).into_http());
    let etag = format!("\"{}\"", hex::encode(Sha256::digest(expected.bytes)));
    let cache = Arc::new(LogoHttpCache::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let (release, released) = tokio::sync::oneshot::channel();
    let first_cache = cache.clone();
    let first_entered = entered.clone();
    let directory = root.path().to_owned();
    let first = tokio::spawn(async move {
        first_cache
            .response_with(
                directory,
                school("School"),
                Vec::new(),
                Arc::new(Semaphore::new(4)),
                move |source| {
                    first_entered.notify_one();
                    checked(released.blocking_recv());
                    source.http_image()
                },
            )
            .await
    });
    entered.notified().await;
    first.abort();
    assert!(first.await.is_err_and(|error| error.is_cancelled()));
    checked(release.send(()));
    let second = checked(
        cache
            .response_with(
                root.path().to_owned(),
                school("School"),
                vec![etag.clone()],
                Arc::new(Semaphore::new(4)),
                |_| panic!("cancelled request must publish its digest"),
            )
            .await,
    );
    assert!(second.image.is_none());
    assert_eq!(second.etag, etag);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hits_bypass_a_colliding_miss_waiting_for_image_capacity() {
    let root = checked(tempfile::tempdir());
    let first_path = root.path().join("School.svg");
    checked(fs::write(&first_path, SVG));
    let cache = Arc::new(LogoHttpCache::default());
    let warm = checked(response(&cache, root.path(), school("School"), None).await);
    let other_name = (0..1000)
        .map(|index| format!("Other{index}"))
        .find(|name| {
            Arc::ptr_eq(
                cache.shard(&first_path),
                cache.shard(&root.path().join(format!("{name}.svg"))),
            )
        })
        .unwrap_or_else(|| panic!("expected a colliding shard"));
    checked(fs::write(
        root.path().join(format!("{other_name}.svg")),
        SVG,
    ));
    let permits = Arc::new(Semaphore::new(4));
    let held = checked(permits.clone().acquire_many_owned(4).await);
    let (other_cache, directory, other_permits) =
        (cache.clone(), root.path().to_owned(), permits.clone());
    let pending = tokio::spawn(async move {
        other_cache
            .response(directory, school(&other_name), Vec::new(), other_permits)
            .await
    });
    checked(
        tokio::time::timeout(Duration::from_secs(1), async {
            while cache.shard(&first_path).work.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await,
    );
    let hit = checked(checked(
        tokio::time::timeout(
            Duration::from_secs(1),
            cache.response_with(
                root.path().to_owned(),
                school("School"),
                vec![warm.etag],
                permits,
                |_| panic!("hit must bypass an unrelated pending miss"),
            ),
        )
        .await,
    ));
    assert!(hit.image.is_none());
    drop(held);
    assert!(checked(checked(pending.await)).image.is_some());
}
