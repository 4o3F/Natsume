use std::{
    fs::{self, File, FileTimes},
    io,
    os::unix::fs::{PermissionsExt as _, symlink},
    sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use super::*;

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><rect width="4" height="3" fill="#f00"/></svg>"##;

fn checked<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    value.unwrap_or_else(|error| panic!("test fixture: {error:?}"))
}

fn validate(cache: &LogoValidationCache, path: &Path, calls: &AtomicUsize) -> Result<(), String> {
    cache.validate_with(path, |source| {
        calls.fetch_add(1, Ordering::SeqCst);
        source.validate()
    })
}

#[test]
fn unchanged_valid_and_invalid_files_are_decoded_once() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    checked(fs::write(&path, SVG));
    let cache = LogoValidationCache::default();
    let calls = AtomicUsize::new(0);
    for _ in 0..3 {
        checked(validate(&cache, &path, &calls));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    checked(fs::write(&path, "not an image"));
    let first = validate(&cache, &path, &calls);
    assert!(first.is_err());
    assert_eq!(validate(&cache, &path, &calls), first);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    checked(fs::write(&path, SVG));
    checked(validate(&cache, &path, &calls));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn preserved_size_and_mtime_do_not_hide_overwrites_or_replacements() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    checked(fs::write(&path, SVG));
    let modified = checked(checked(fs::metadata(&path)).modified());
    let cache = LogoValidationCache::default();
    checked(cache.validate(&path));
    let mut invalid = SVG.as_bytes().to_vec();
    invalid[0] = b'!';
    thread::sleep(Duration::from_millis(10));
    checked(fs::write(&path, &invalid));
    checked(
        checked(File::options().write(true).open(&path))
            .set_times(FileTimes::new().set_modified(modified)),
    );
    assert_eq!(checked(fs::metadata(&path)).len(), SVG.len() as u64);
    assert_eq!(checked(checked(fs::metadata(&path)).modified()), modified);
    assert!(cache.validate(&path).is_err());

    let replacement = root.path().join("replacement.svg");
    checked(fs::write(&replacement, SVG));
    checked(
        checked(File::options().write(true).open(&replacement))
            .set_times(FileTimes::new().set_modified(modified)),
    );
    checked(fs::rename(replacement, &path));
    checked(cache.validate(&path));
}

#[test]
fn replacement_during_validation_does_not_validate_the_new_path_with_old_bytes() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    let replacement = root.path().join("replacement.svg");
    checked(fs::write(&path, SVG));
    checked(fs::write(&replacement, "invalid"));
    let cache = LogoValidationCache::default();
    checked(cache.validate_with(&path, |source| {
        checked(fs::rename(&replacement, &path));
        source.validate()
    }));
    assert!(cache.validate(&path).is_err());
}

#[test]
fn modifications_during_validation_are_not_cached() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    checked(fs::write(&path, SVG));
    let cache = LogoValidationCache::default();
    checked(cache.validate_with(&path, |source| {
        source.validate()?;
        checked(fs::write(&path, "changed"));
        Ok(())
    }));
    let entry = cache.entry(&path);
    assert!(
        entry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    );
    assert!(cache.validate(&path).is_err());
}

#[test]
fn concurrent_misses_share_one_decoder() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    checked(fs::write(&path, SVG));
    let cache = LogoValidationCache::default();
    let calls = AtomicUsize::new(0);
    let barrier = Barrier::new(8);
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                barrier.wait();
                checked(cache.validate_with(&path, |source| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    source.validate()
                }));
            });
        }
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn removal_symlinks_and_access_changes_do_not_use_cached_success() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    checked(fs::write(&path, SVG));
    let cache = LogoValidationCache::default();
    checked(cache.validate(&path));
    checked(fs::remove_file(&path));
    assert!(cache.validate(&path).is_err());
    let other = root.path().join("other.svg");
    checked(fs::write(&other, SVG));
    checked(symlink(&other, &path));
    assert!(cache.validate(&path).is_err());
    checked(fs::remove_file(&path));
    checked(fs::write(&path, SVG));
    checked(cache.validate(&path));
    checked(fs::set_permissions(
        &path,
        fs::Permissions::from_mode(0o000),
    ));
    if File::open(&path).is_err() {
        assert!(cache.validate(&path).is_err());
    }
    checked(fs::set_permissions(
        &path,
        fs::Permissions::from_mode(0o600),
    ));
    checked(cache.validate(&path));
    checked(fs::set_permissions(
        root.path(),
        fs::Permissions::from_mode(0o000),
    ));
    let unreadable = File::open(&path).is_err();
    let result = cache.validate(&path);
    checked(fs::set_permissions(
        root.path(),
        fs::Permissions::from_mode(0o700),
    ));
    if unreadable {
        assert!(result.is_err());
    }
    checked(cache.validate(&path));
}

#[test]
fn transient_io_and_large_errors_are_not_retained() {
    let root = checked(tempfile::tempdir());
    let path = root.path().join("logo.svg");
    checked(fs::write(&path, SVG));
    for failure in [
        LogoError::Read(io::Error::other("transient")),
        LogoError::Encode("x".repeat(MAX_CACHED_ERROR_BYTES + 1)),
    ] {
        let cache = LogoValidationCache::default();
        assert!(cache.validate_with(&path, |_| Err(failure)).is_err());
        let entry = cache.entry(&path);
        assert!(
            entry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
        );
        checked(cache.validate(&path));
    }
}

#[test]
fn entries_are_bounded_and_recent_or_inflight_entries_survive_eviction() {
    let cache = LogoValidationCache::default();
    let first = PathBuf::from("0.svg");
    let pinned = cache.entry(&first);
    for index in 1..MAX_ENTRIES {
        drop(cache.entry(&PathBuf::from(format!("{index}.svg"))));
    }
    drop(cache.entry(Path::new("1.svg")));
    drop(cache.entry(Path::new("new.svg")));
    let entries = cache.entries.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(entries.len(), MAX_ENTRIES);
    assert!(Arc::ptr_eq(&entries[&first].validation, &pinned));
    assert!(entries.contains_key(Path::new("1.svg")));
    assert!(!entries.contains_key(Path::new("2.svg")));
    assert!(entries.contains_key(Path::new("new.svg")));
}
