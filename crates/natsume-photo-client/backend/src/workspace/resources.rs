//! Display resources only; this module does not track photographic candidates.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, ensure};

use super::Result;

#[cfg(test)]
mod tests;
use super::db::{DecodeSnafu, IoSnafu, PathSnafu, ResourceSnafu, ResourcesJsonSnafu};
use crate::roster::Roster;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    #[serde(default)]
    school_logos: Vec<SchoolLogo>,
    default_image: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchoolLogo {
    organization_id: i64,
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ImageRef {
    pub path: String,
    pub sha256: String,
}

impl ImageRef {
    pub(super) fn from_columns(
        path: Option<String>,
        sha256: Option<String>,
    ) -> Result<Option<Self>> {
        match (path, sha256) {
            (None, None) => Ok(None),
            (Some(path), Some(sha256)) => Ok(Some(Self { path, sha256 })),
            _ => ResourceSnafu.fail(),
        }
    }

    pub(super) fn verify(&self, root: &Path) -> Result<PathBuf> {
        // Cached names are generated from content; never accept arbitrary DB paths.
        let png = format!("resources/{}.png", self.sha256);
        let jpg = format!("resources/{}.jpg", self.sha256);
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            ResourceSnafu
        );
        ensure!(self.path == png || self.path == jpg, PathSnafu);
        let path = std::fs::canonicalize(root.join(&self.path)).context(IoSnafu)?;
        ensure!(path.starts_with(root), PathSnafu);
        let bytes = read_image(&path)?;
        ensure!(Self::for_bytes(&bytes) == *self, ResourceSnafu);
        Ok(path)
    }

    fn for_bytes(bytes: &[u8]) -> Self {
        let sha256 = hex::encode(Sha256::digest(bytes));
        let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "png"
        } else {
            "jpg"
        };
        Self {
            path: format!("resources/{sha256}.{extension}"),
            sha256,
        }
    }
}

#[derive(Default)]
pub(super) struct Cache {
    pub logos: BTreeMap<i64, ImageRef>,
    pub default_image: Option<ImageRef>,
}

pub(super) struct Prepared {
    cache: Cache,
    files: BTreeMap<String, (PathBuf, ImageRef)>,
}

impl Prepared {
    pub(super) fn load(manifest: &Path, roster: &Roster) -> Result<Self> {
        let bytes = std::fs::read(manifest).context(IoSnafu)?;
        let input: Inputs = serde_json::from_slice(&bytes).context(ResourcesJsonSnafu)?;
        let source = std::fs::canonicalize(
            manifest
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .context(IoSnafu)?;
        let organizations: BTreeSet<_> = roster
            .teams()
            .iter()
            .map(|team| team.organization_id)
            .collect();
        let mut prepared = Self {
            cache: Cache::default(),
            files: BTreeMap::new(),
        };
        for logo in input.school_logos {
            ensure!(
                organizations.contains(&logo.organization_id)
                    && !prepared.cache.logos.contains_key(&logo.organization_id),
                ResourceSnafu
            );
            let reference = prepared.add(&source, &logo.path)?;
            prepared.cache.logos.insert(logo.organization_id, reference);
        }
        if let Some(path) = input.default_image {
            prepared.cache.default_image = Some(prepared.add(&source, &path)?);
        }
        Ok(prepared)
    }

    fn add(&mut self, source: &Path, relative: &Path) -> Result<ImageRef> {
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            PathSnafu
        );
        let path = std::fs::canonicalize(source.join(relative)).context(IoSnafu)?;
        ensure!(path.starts_with(source), PathSnafu);
        let bytes = read_image(&path)?;
        let reference = ImageRef::for_bytes(&bytes);
        self.files
            .insert(reference.path.clone(), (path, reference.clone()));
        Ok(reference)
    }

    pub(super) fn copy_to(self, root: &Path) -> Result<Cache> {
        if !self.files.is_empty() {
            std::fs::create_dir(root.join("resources")).context(IoSnafu)?;
        }
        for (source, expected) in self.files.values() {
            let bytes = read_image(source)?;
            ensure!(ImageRef::for_bytes(&bytes) == *expected, ResourceSnafu);
            let mut file = std::fs::File::create_new(root.join(&expected.path)).context(IoSnafu)?;
            file.write_all(&bytes).context(IoSnafu)?;
            file.sync_all().context(IoSnafu)?;
            drop(file);
            expected.verify(root)?;
        }
        Ok(self.cache)
    }
}

fn read_image(path: &Path) -> Result<Vec<u8>> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .context(IoSnafu)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= LIMIT, ResourceSnafu);
    let mut reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .context(IoSnafu)?;
    ensure!(
        matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ),
        ResourceSnafu
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().context(DecodeSnafu)?;
    Ok(bytes)
}
