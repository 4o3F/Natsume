//! Content-addressed school logos; missing photos never use a default image.
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Snafu)]
pub enum ResourceError {
    #[snafu(display("school logo file operation failed"))]
    Io { source: std::io::Error },
    #[snafu(display("school logo manifest has invalid fields or JSON"))]
    Manifest { source: serde_json::Error },
    #[snafu(display("school logo path escapes its permitted directory"))]
    Path,
    #[snafu(display("school logo path and hash must both be present or absent"))]
    IncompleteReference,
    #[snafu(display("school logo hash is not lowercase SHA-256"))]
    InvalidHash,
    #[snafu(display("school logo content does not match its recorded hash"))]
    HashMismatch,
    #[snafu(display("school logo exceeds the 16 MiB byte limit"))]
    ByteLimit,
    #[snafu(display("school logo is not a PNG or JPEG image"))]
    Format,
    #[snafu(display("school logo cannot be decoded within the image limits"))]
    Decode { source: image::ImageError },
    #[snafu(display("school logo references an organization outside the roster"))]
    UnknownSchool,
    #[snafu(display("school logo manifest repeats an organization"))]
    DuplicateSchool,
}
type Result<T> = std::result::Result<T, ResourceError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    #[serde(default)]
    school_logos: Vec<SchoolLogo>,
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
            _ => IncompleteReferenceSnafu.fail(),
        }
    }
    pub(super) fn verify(&self, root: &Path) -> Result<PathBuf> {
        ensure!(
            self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            InvalidHashSnafu
        );
        ensure!(
            self.path == format!("resources/{}.png", self.sha256)
                || self.path == format!("resources/{}.jpg", self.sha256),
            PathSnafu
        );
        let path = std::fs::canonicalize(root.join(&self.path)).context(IoSnafu)?;
        ensure!(path.starts_with(root), PathSnafu);
        ensure!(
            Self::for_bytes(&read_image(&path)?) == *self,
            HashMismatchSnafu
        );
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
}
pub(super) struct Prepared {
    cache: Cache,
    files: BTreeMap<String, (PathBuf, ImageRef)>,
}
impl Prepared {
    pub(super) fn load(manifest: &Path, organizations: &BTreeSet<i64>) -> Result<Self> {
        let input: Inputs = serde_json::from_slice(&std::fs::read(manifest).context(IoSnafu)?)
            .context(ManifestSnafu)?;
        let source = std::fs::canonicalize(
            manifest
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .context(IoSnafu)?;
        let mut prepared = Self {
            cache: Cache::default(),
            files: BTreeMap::new(),
        };
        for logo in input.school_logos {
            ensure!(
                organizations.contains(&logo.organization_id),
                UnknownSchoolSnafu
            );
            ensure!(
                !prepared.cache.logos.contains_key(&logo.organization_id),
                DuplicateSchoolSnafu
            );
            ensure!(
                !logo.path.as_os_str().is_empty()
                    && logo
                        .path
                        .components()
                        .all(|c| matches!(c, Component::Normal(_))),
                PathSnafu
            );
            let path = std::fs::canonicalize(source.join(logo.path)).context(IoSnafu)?;
            ensure!(path.starts_with(&source), PathSnafu);
            let reference = ImageRef::for_bytes(&read_image(&path)?);
            prepared
                .files
                .insert(reference.path.clone(), (path, reference.clone()));
            prepared.cache.logos.insert(logo.organization_id, reference);
        }
        Ok(prepared)
    }
    pub(super) fn copy_to(self, root: &Path) -> Result<Cache> {
        if !self.files.is_empty() {
            std::fs::create_dir(root.join("resources")).context(IoSnafu)?;
        }
        for (source, reference) in self.files.values() {
            let bytes = read_image(source)?;
            ensure!(ImageRef::for_bytes(&bytes) == *reference, HashMismatchSnafu);
            let mut file =
                std::fs::File::create_new(root.join(&reference.path)).context(IoSnafu)?;
            file.write_all(&bytes).context(IoSnafu)?;
            file.sync_all().context(IoSnafu)?;
            drop(file);
            reference.verify(root)?;
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
    ensure!(bytes.len() as u64 <= LIMIT, ByteLimitSnafu);
    let mut reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .context(IoSnafu)?;
    ensure!(
        matches!(
            reader.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ),
        FormatSnafu
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().context(DecodeSnafu)?;
    Ok(bytes)
}
#[cfg(test)]
#[path = "../../tests/unit/workspace_resources.rs"]
mod tests;
