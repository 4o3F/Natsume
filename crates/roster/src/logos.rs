use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, Cursor, Read, Seek},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use resvg::{tiny_skia, usvg};

use rustix::fs::{Mode, OFlags};

const MAX_LOGO_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LOGO_DIMENSION: u32 = 4096;
const MAX_LOGO_PIXELS: u64 = 4 * 1024 * 1024;

/// Identity and change metadata for an opened logo, not a content digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogoFingerprint {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

/// An opened logo whose metadata and validation use the same file descriptor.
pub struct LogoFile {
    file: File,
}

impl LogoFile {
    /// Open a readable regular file without following a final symlink.
    ///
    /// # Errors
    /// Rejects unreadable/nonregular files and files exceeding the byte limit.
    pub fn open(path: &Path) -> Result<Self, LogoError> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            if error == rustix::io::Errno::LOOP {
                LogoError::NotRegularFile
            } else {
                LogoError::Read(error.into())
            }
        })?;
        let source = Self {
            file: File::from(descriptor),
        };
        source.fingerprint()?;
        Ok(source)
    }

    /// Inspect the currently opened file, even if its path has been replaced.
    ///
    /// # Errors
    /// Returns metadata errors, nonregular-file errors and byte-limit errors.
    pub fn fingerprint(&self) -> Result<LogoFingerprint, LogoError> {
        let metadata = self.file.metadata().map_err(LogoError::Read)?;
        if !metadata.is_file() {
            return Err(LogoError::NotRegularFile);
        }
        if metadata.len() > MAX_LOGO_BYTES {
            return Err(LogoError::TooLarge);
        }
        Ok(LogoFingerprint {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }

    /// Fully validate this opened file with the normal content and image limits.
    ///
    /// # Errors
    /// Returns the same read/format/dimension/resource errors as [`validate_logo`].
    pub fn validate(&mut self) -> Result<(), LogoError> {
        decode_logo(&mut self.file).map(|_| ())
    }
}

/// A directory snapshot indexed by exact filename stem. No recursive scanning.
pub struct LogoDirectory {
    files: BTreeMap<OsString, Vec<PathBuf>>,
}

/// A school must resolve to exactly one supported image filename.
#[derive(Debug, PartialEq, Eq)]
pub enum LogoMatch {
    Missing,
    Unique(PathBuf),
    Ambiguous(Vec<PathBuf>),
}

impl LogoDirectory {
    /// Index PNG, JPEG, WebP and SVG filenames, retaining duplicates for diagnostics.
    ///
    /// # Errors
    /// Returns directory listing errors; individual image failures are reported
    /// separately by [`validate_logo`].
    pub fn open(directory: &Path) -> io::Result<Self> {
        let mut files: BTreeMap<OsString, Vec<PathBuf>> = BTreeMap::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if !supported_extension(&path) {
                continue;
            }
            if let Some(stem) = path.file_stem() {
                files.entry(stem.to_owned()).or_default().push(path);
            }
        }
        for paths in files.values_mut() {
            paths.sort();
        }
        Ok(Self { files })
    }

    /// Match either complete school name, without aliases or punctuation folding.
    #[must_use]
    pub fn resolve(&self, name_zh: &str, name_en: &str) -> LogoMatch {
        let mut candidates = Vec::new();
        for name in [name_zh, name_en] {
            if !name.is_empty()
                && let Some(paths) = self.files.get(std::ffi::OsStr::new(name))
            {
                candidates.extend(paths.iter().cloned());
            }
        }
        candidates.sort();
        candidates.dedup();
        match candidates.len() {
            0 => LogoMatch::Missing,
            1 => LogoMatch::Unique(candidates.remove(0)),
            _ => LogoMatch::Ambiguous(candidates),
        }
    }
}

#[derive(Debug)]
pub enum LogoError {
    Read(io::Error),
    NotRegularFile,
    TooLarge,
    UnsupportedFormat,
    Dimensions,
    Decode(image::ImageError),
    Svg(usvg::Error),
    Encode(String),
    SvgResources,
    SvgFont,
}

impl fmt::Display for LogoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(f, "cannot read image: {error}"),
            Self::NotRegularFile => {
                f.write_str("expected a regular file, not a symlink or directory")
            }
            Self::TooLarge => f.write_str("image exceeds the 8 MiB limit"),
            Self::UnsupportedFormat => f.write_str("expected PNG, JPEG, WebP or SVG content"),
            Self::Dimensions => {
                f.write_str("image exceeds 4096 pixels per side or 4194304 pixels in total")
            }
            Self::Decode(error) => write!(f, "cannot decode image: {error}"),
            Self::Encode(error) => write!(f, "cannot encode PNG: {error}"),
            Self::Svg(error) => write!(f, "cannot parse SVG: {error}"),
            Self::SvgResources => f.write_str("SVG images must be embedded, valid PNG, JPEG or WebP within the logo limits; external files and URLs are not loaded"),
            Self::SvgFont => f.write_str("SVG text has no usable font; install the required fonts or convert the text to paths"),
        }
    }
}

impl Error for LogoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Svg(error) => Some(error),
            _ => None,
        }
    }
}

/// Validate actual image content and decoding limits without modifying the file.
///
/// # Errors
/// Rejects unreadable or nonregular files, unsupported image content,
/// malformed image data, and images exceeding the documented limits.
pub fn validate_logo(path: &Path) -> Result<(), LogoError> {
    read_logo(path).map(|_| ())
}

/// A validated logo, decoded once for HTTP delivery or PNG export.
pub struct DecodedLogo(LogoContent);

enum LogoContent {
    Raster {
        source: Vec<u8>,
        format: ImageFormat,
        image: DynamicImage,
    },
    Svg(tiny_skia::Pixmap),
}

/// The actual image bytes and MIME type, independent of the source extension.
pub struct LogoImage {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

impl DecodedLogo {
    /// Preserve raster source bytes; rasterize SVG for safe browser delivery.
    ///
    /// # Errors
    /// Returns PNG encoding failures for SVG sources.
    pub fn into_http(self) -> Result<LogoImage, LogoError> {
        match self.0 {
            LogoContent::Raster { source, format, .. } => Ok(LogoImage {
                bytes: source,
                content_type: format.to_mime_type(),
            }),
            LogoContent::Svg(image) => Ok(LogoImage {
                bytes: image
                    .encode_png()
                    .map_err(|error| LogoError::Encode(error.to_string()))?,
                content_type: "image/png",
            }),
        }
    }

    /// Encode a PNG without resizing, cropping, or changing source files.
    ///
    /// # Errors
    /// Returns PNG encoding failures.
    pub fn into_png(self) -> Result<Vec<u8>, LogoError> {
        match self.0 {
            LogoContent::Raster { image, .. } => {
                let mut output = Cursor::new(Vec::new());
                image
                    .write_to(&mut output, ImageFormat::Png)
                    .map_err(|error| LogoError::Encode(error.to_string()))?;
                Ok(output.into_inner())
            }
            LogoContent::Svg(image) => image
                .encode_png()
                .map_err(|error| LogoError::Encode(error.to_string())),
        }
    }
}

/// Read and decode bounded image content without following symlinks.
///
/// # Errors
/// Rejects unreadable/nonregular files, malformed or unsupported content and
/// images exceeding the same byte, dimension and resource limits as preflight.
pub fn read_logo(path: &Path) -> Result<DecodedLogo, LogoError> {
    let mut source = LogoFile::open(path)?;
    decode_logo(&mut source.file)
}

fn decode_logo(file: &mut File) -> Result<DecodedLogo, LogoError> {
    file.rewind().map_err(LogoError::Read)?;
    let mut bytes = Vec::new();
    file.take(MAX_LOGO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(LogoError::Read)?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        return Err(LogoError::TooLarge);
    }
    let content = match image::guess_format(&bytes) {
        Ok(format) => LogoContent::Raster {
            image: decode_raster(&bytes, format)?,
            source: bytes,
            format,
        },
        Err(_) => LogoContent::Svg(render_svg(&bytes)?),
    };
    Ok(DecodedLogo(content))
}

fn decode_raster(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, LogoError> {
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(LogoError::UnsupportedFormat);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(LogoError::Decode)?;
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height)?;
    DynamicImage::from_decoder(decoder).map_err(LogoError::Decode)
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), LogoError> {
    if width > MAX_LOGO_DIMENSION
        || height > MAX_LOGO_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_LOGO_PIXELS
    {
        return Err(LogoError::Dimensions);
    }
    Ok(())
}

fn render_svg(bytes: &[u8]) -> Result<tiny_skia::Pixmap, LogoError> {
    let text = std::str::from_utf8(bytes).map_err(|_| LogoError::UnsupportedFormat)?;
    if !text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with('<')
    {
        return Err(LogoError::UnsupportedFormat);
    }
    let document = usvg::roxmltree::Document::parse_with_options(
        text,
        usvg::roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: 100_000,
            ..Default::default()
        },
    )
    .map_err(|error| LogoError::Svg(usvg::Error::ParsingFailed(error)))?;
    if !document
        .root_element()
        .has_tag_name(("http://www.w3.org/2000/svg", "svg"))
    {
        return Err(LogoError::UnsupportedFormat);
    }
    let invalid_resource = AtomicBool::new(false);
    let missing_font = AtomicBool::new(false);
    let mut options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            // The default resolver reads local paths. Logos must be self-contained.
            resolve_string: Box::new(|_, _| {
                invalid_resource.store(true, Ordering::Relaxed);
                None
            }),
            resolve_data: Box::new(|_, data, _| {
                let kind = image::guess_format(&data).ok().and_then(|format| {
                    if data.len() as u64 > MAX_LOGO_BYTES || decode_raster(&data, format).is_err() {
                        return None;
                    }
                    match format {
                        ImageFormat::Png => Some(usvg::ImageKind::PNG(data)),
                        ImageFormat::Jpeg => Some(usvg::ImageKind::JPEG(data)),
                        ImageFormat::WebP => Some(usvg::ImageKind::WEBP(data)),
                        _ => None,
                    }
                });
                if kind.is_none() {
                    invalid_resource.store(true, Ordering::Relaxed);
                }
                kind
            }),
        },
        ..usvg::Options::default()
    };
    if document
        .descendants()
        .any(|node| node.has_tag_name(("http://www.w3.org/2000/svg", "text")))
    {
        static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
            let mut fonts = usvg::fontdb::Database::new();
            fonts.load_system_fonts();
            Arc::new(fonts)
        });
        options.fontdb = Arc::clone(&FONTS);
        let resolver = usvg::FontResolver::default();
        let missing_font = &missing_font;
        options.font_resolver = usvg::FontResolver {
            select_font: Box::new(move |font, database| {
                let selected = (resolver.select_font)(font, database);
                missing_font.fetch_or(selected.is_none(), Ordering::Relaxed);
                selected
            }),
            select_fallback: Box::new(move |character, used, database| {
                let selected = (resolver.select_fallback)(character, used, database);
                missing_font.fetch_or(selected.is_none(), Ordering::Relaxed);
                selected
            }),
        };
    }
    let tree = usvg::Tree::from_xmltree(&document, &options).map_err(LogoError::Svg)?;
    if invalid_resource.load(Ordering::Relaxed) {
        return Err(LogoError::SvgResources);
    }
    if missing_font.load(Ordering::Relaxed) {
        return Err(LogoError::SvgFont);
    }
    let size = tree.size().to_int_size();
    validate_dimensions(size.width(), size.height())?;
    let mut pixmap =
        tiny_skia::Pixmap::new(size.width(), size.height()).ok_or(LogoError::Dimensions)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "webp", "svg"]
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

#[cfg(test)]
mod tests;
