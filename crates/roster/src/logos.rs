use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use resvg::{tiny_skia, usvg};

use crate::Organization;

const MAX_LOGO_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LOGO_DIMENSION: u32 = 4096;
const MAX_LOGO_PIXELS: u64 = 4 * 1024 * 1024;

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
    pub fn resolve(&self, organization: &Organization) -> LogoMatch {
        let mut candidates = Vec::new();
        for name in [organization.name_zh(), organization.name_en()] {
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
    let metadata = fs::symlink_metadata(path).map_err(LogoError::Read)?;
    if !metadata.is_file() {
        return Err(LogoError::NotRegularFile);
    }
    if metadata.len() > MAX_LOGO_BYTES {
        return Err(LogoError::TooLarge);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(LogoError::Read)?
        .take(MAX_LOGO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(LogoError::Read)?;
    if bytes.len() as u64 > MAX_LOGO_BYTES {
        return Err(LogoError::TooLarge);
    }
    match image::guess_format(&bytes) {
        Ok(format) => validate_raster(&bytes, format),
        Err(_) => render_svg(&bytes).map(|_| ()),
    }
}

fn validate_raster(bytes: &[u8], format: ImageFormat) -> Result<(), LogoError> {
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
    DynamicImage::from_decoder(decoder).map_err(LogoError::Decode)?;
    Ok(())
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
                    if data.len() as u64 > MAX_LOGO_BYTES || validate_raster(&data, format).is_err()
                    {
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
