use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use natsume_roster::{LogoError, LogoFile, LogoFingerprint};

const MAX_ENTRIES: usize = 1024;
const MAX_CACHED_ERROR_BYTES: usize = 4096;

#[derive(Default)]
pub(super) struct LogoValidationCache {
    entries: Mutex<HashMap<PathBuf, Entry>>,
}

struct Entry {
    last_used: Instant,
    validation: Arc<Mutex<Option<ValidatedLogo>>>,
}

struct ValidatedLogo {
    fingerprint: LogoFingerprint,
    result: Result<(), String>,
}

impl LogoValidationCache {
    pub(super) fn validate(&self, path: &Path) -> Result<(), String> {
        self.validate_with(path, LogoFile::validate)
    }

    fn entry(&self, path: &Path) -> Arc<Mutex<Option<ValidatedLogo>>> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(path) {
            entry.last_used = Instant::now();
            return Arc::clone(&entry.validation);
        }
        if entries.len() >= MAX_ENTRIES {
            let oldest = entries
                .iter()
                .filter(|(_, entry)| Arc::strong_count(&entry.validation) == 1)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(path, _)| path.clone());
            if let Some(oldest) = oldest {
                entries.remove(&oldest);
            } else {
                // Never evict a live validation and create a duplicate decoder.
                // If every entry is pinned, this result simply isn't retained.
                return Arc::new(Mutex::new(None));
            }
        }
        let validation = Arc::new(Mutex::new(None));
        entries.insert(
            path.to_owned(),
            Entry {
                last_used: Instant::now(),
                validation: Arc::clone(&validation),
            },
        );
        validation
    }

    fn validate_with(
        &self,
        path: &Path,
        validate: impl FnOnce(&mut LogoFile) -> Result<(), LogoError>,
    ) -> Result<(), String> {
        let entry = self.entry(path);
        // Runs only inside the existing bounded image workers. The map lock is
        // not held across I/O; callers for the same path share one validation.
        let mut cached = entry.lock().unwrap_or_else(PoisonError::into_inner);
        let mut source = LogoFile::open(path).map_err(|error| error.to_string())?;
        let before = source.fingerprint().map_err(|error| error.to_string())?;
        if let Some(cached) = cached.as_ref()
            && cached.fingerprint == before
        {
            return cached.result.clone();
        }
        *cached = None;
        let result = validate(&mut source);
        let transient = matches!(result, Err(LogoError::Read(_)));
        let result = result.map_err(|error| error.to_string());
        if !transient
            && result
                .as_ref()
                .err()
                .is_none_or(|error| error.len() <= MAX_CACHED_ERROR_BYTES)
            && source.fingerprint().is_ok_and(|after| before == after)
        {
            *cached = Some(ValidatedLogo {
                fingerprint: before,
                result: result.clone(),
            });
        }
        result
    }
}

#[cfg(test)]
mod tests;
