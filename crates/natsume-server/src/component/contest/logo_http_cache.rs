use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use super::{
    ExportError, OrganizationDetails,
    logo_io::{LogoError, LogoFile, LogoFingerprint, LogoImage, LogoMatch},
    logos::{logo_error, open_directory, resolve},
};

// Each shard retains at most 64 ETags. Fixed async locks also bound the
// single-flight state, including when every cache entry is in use.
const SHARDS: usize = 16;
const ENTRIES_PER_SHARD: usize = 64;
const MAX_RESOLUTIONS: usize = 1024;

#[derive(Default)]
struct EtagShard {
    entries: Mutex<HashMap<PathBuf, CachedEtag>>,
    work: Arc<AsyncMutex<()>>,
}

pub(super) struct LogoHttpCache {
    resolutions: Arc<AsyncMutex<Resolutions>>,
    etags: [Arc<EtagShard>; SHARDS],
}

impl Default for LogoHttpCache {
    fn default() -> Self {
        Self {
            resolutions: Arc::new(AsyncMutex::new(Resolutions::default())),
            etags: std::array::from_fn(|_| Arc::new(EtagShard::default())),
        }
    }
}

#[derive(Default)]
struct Resolutions {
    directory: Option<(PathBuf, LogoFingerprint)>,
    entries: HashMap<(String, String), (PathBuf, Instant)>,
}

struct CachedEtag {
    fingerprint: LogoFingerprint,
    etag: String,
    last_used: Instant,
}

pub(crate) struct LogoResponse {
    pub(crate) etag: String,
    pub(crate) image: Option<LogoImage>,
}

impl LogoHttpCache {
    pub(super) async fn response(
        &self,
        directory: PathBuf,
        organization: OrganizationDetails,
        conditions: Vec<String>,
        image_work: Arc<Semaphore>,
    ) -> Result<LogoResponse, ExportError> {
        self.response_with(
            directory,
            organization,
            conditions,
            image_work,
            LogoFile::http_image,
        )
        .await
    }

    async fn response_with(
        &self,
        directory: PathBuf,
        organization: OrganizationDetails,
        conditions: Vec<String>,
        image_work: Arc<Semaphore>,
        read: impl FnOnce(&mut LogoFile) -> Result<LogoImage, LogoError> + Send + 'static,
    ) -> Result<LogoResponse, ExportError> {
        // Serialize directory snapshots without blocking worker threads on a
        // lock. The owned guard also survives cancellation during a scan.
        let mut resolutions = Arc::clone(&self.resolutions).lock_owned().await;
        let school = organization.clone();
        let path = worker(move || resolutions.resolve(&directory, &school)).await?;
        let shard = Arc::clone(self.shard(&path));
        if let (_, Some(etag)) =
            inspect(Arc::clone(&shard), path.clone(), organization.clone()).await?
            && matches_etag(&conditions, &etag)
        {
            return Ok(LogoResponse { etag, image: None });
        }
        // Cache hits bypass admission even if another path in this shard is
        // waiting for an image permit. Only misses and 200 responses serialize.
        let work = Arc::clone(&shard.work).lock_owned().await;
        let (fingerprint, previous) =
            inspect(Arc::clone(&shard), path.clone(), organization.clone()).await?;
        if let Some(etag) = previous.as_ref()
            && matches_etag(&conditions, etag)
        {
            return Ok(LogoResponse {
                etag: etag.clone(),
                image: None,
            });
        }

        let permit = image_work
            .acquire_owned()
            .await
            .map_err(|source| ExportError::WorkerUnavailable { source })?;
        let source_path = path.clone();
        // The worker owns the computation lock and the image permit. A dropped
        // HTTP future cannot release single-flight admission before work ends.
        worker(move || {
            let _permit = permit;
            let _work = work;
            let load = || {
                let mut source = LogoFile::open(&source_path)?;
                let before = source.fingerprint()?;
                let image = read(&mut source)?;
                let stable = source.fingerprint().is_ok_and(|after| after == before)
                    && LogoFile::path_fingerprint(&source_path).is_ok_and(|after| after == before);
                // A 200 still validates current bytes. Reuse the digest only if
                // those bytes have the same metadata as the cached response.
                let etag = previous
                    .filter(|_| stable && before == fingerprint)
                    .unwrap_or_else(|| {
                        format!("\"{}\"", hex::encode(Sha256::digest(&image.bytes)))
                    });
                Ok((
                    LogoResponse {
                        etag,
                        image: Some(image),
                    },
                    stable.then_some(before),
                ))
            };
            let (mut response, stable) =
                load().map_err(|error| logo_error(&organization, &source_path, error))?;
            let mut entries = shard.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(fingerprint) = stable {
                if entries.len() >= ENTRIES_PER_SHARD && !entries.contains_key(&path) {
                    let oldest = entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_used)
                        .map(|(path, _)| path.clone());
                    if let Some(oldest) = oldest {
                        entries.remove(&oldest);
                    }
                }
                entries.insert(
                    path,
                    CachedEtag {
                        fingerprint,
                        etag: response.etag.clone(),
                        last_used: Instant::now(),
                    },
                );
            } else {
                entries.remove(&path);
            }
            if matches_etag(&conditions, &response.etag) {
                response.image = None;
            }
            Ok(response)
        })
        .await
    }
    fn shard(&self, path: &Path) -> &Arc<EtagShard> {
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        &self.etags[usize::from(hasher.finish().to_le_bytes()[0]) % SHARDS]
    }
}

impl Resolutions {
    fn resolve(
        &mut self,
        directory: &Path,
        organization: &OrganizationDetails,
    ) -> Result<PathBuf, ExportError> {
        // Directory identity, size, mtime and ctime detect entry additions,
        // removals, renames and directory replacement. School names come from
        // the current roster on every request and are part of the lookup key.
        // Changes hidden by timestamp granularity or remote metadata caching
        // remain subject to the filesystem's metadata visibility guarantees.
        let before = match fs::metadata(directory) {
            Ok(metadata) => LogoFingerprint::from_metadata(&metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ExportError::LogoUnavailable);
            }
            Err(source) => return Err(ExportError::LogoDirectory { source }),
        };
        let key = (organization.name_zh.clone(), organization.name_en.clone());
        let stamp = (directory.to_owned(), before);
        if self.directory.as_ref() == Some(&stamp)
            && let Some((path, last_used)) = self.entries.get_mut(&key)
        {
            *last_used = Instant::now();
            return Ok(path.clone());
        }
        let index = open_directory(directory)?;
        let LogoMatch::Unique(path) = resolve(index.as_ref(), organization) else {
            return Err(ExportError::LogoUnavailable);
        };
        if fs::metadata(directory)
            .is_ok_and(|metadata| LogoFingerprint::from_metadata(&metadata) == before)
        {
            if self.directory.as_ref() != Some(&stamp) {
                self.entries.clear();
                self.directory = Some(stamp);
            }
            if self.entries.len() >= MAX_RESOLUTIONS && !self.entries.contains_key(&key) {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, (_, used))| *used)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    self.entries.remove(&oldest);
                }
            }
            self.entries.insert(key, (path.clone(), Instant::now()));
        }
        Ok(path)
    }
}

async fn inspect(
    shard: Arc<EtagShard>,
    path: PathBuf,
    organization: OrganizationDetails,
) -> Result<(LogoFingerprint, Option<String>), ExportError> {
    worker(move || {
        let fingerprint = LogoFile::path_fingerprint(&path)
            .map_err(|error| logo_error(&organization, &path, error))?;
        let mut entries = shard.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = entries
            .get_mut(&path)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| {
                entry.last_used = Instant::now();
                entry.etag.clone()
            });
        Ok((fingerprint, previous))
    })
    .await
}

fn matches_etag(conditions: &[String], etag: &str) -> bool {
    conditions
        .iter()
        .flat_map(|value| value.split(','))
        .any(|value| {
            let value = value.trim();
            value == "*" || value.strip_prefix("W/").unwrap_or(value) == etag
        })
}

async fn worker<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ExportError> + Send + 'static,
) -> Result<T, ExportError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|source| ExportError::Worker {
            source: source.into(),
        })?
}

#[cfg(test)]
mod tests;
