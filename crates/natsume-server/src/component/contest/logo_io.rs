use snafu::Snafu;
use std::{
    collections::BTreeMap,
    ffi::OsString,
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
pub(super) struct LogoFingerprint {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

/// An opened logo whose metadata and validation use the same file descriptor.
pub(super) struct LogoFile {
    file: File,
}

impl LogoFile {
    /// Open a readable regular file without following a final symlink.
    ///
    /// # Errors
    /// Rejects unreadable/nonregular files and files exceeding the byte limit.
    pub(super) fn open(path: &Path) -> Result<Self, LogoError> {
        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            if error == rustix::io::Errno::LOOP {
                LogoError::NotRegularFile
            } else {
                LogoError::Read {
                    source: error.into(),
                }
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
    pub(super) fn fingerprint(&self) -> Result<LogoFingerprint, LogoError> {
        let metadata = self
            .file
            .metadata()
            .map_err(|source| LogoError::Read { source })?;
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
    /// Returns the same read/format/dimension/resource errors as [`read_logo`].
    pub(super) fn validate(&mut self) -> Result<(), LogoError> {
        decode_logo(&mut self.file).map(|_| ())
    }
}

/// A directory snapshot indexed by exact filename stem. No recursive scanning.
pub(super) struct LogoDirectory {
    files: BTreeMap<OsString, Vec<PathBuf>>,
}

/// A school must resolve to exactly one supported image filename.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LogoMatch {
    Missing,
    Unique(PathBuf),
    Ambiguous(Vec<PathBuf>),
}

impl LogoDirectory {
    /// Index PNG, JPEG, WebP and SVG filenames, retaining duplicates for diagnostics.
    ///
    /// # Errors
    /// Returns directory listing errors; individual image failures are reported
    /// separately by [`read_logo`].
    pub(super) fn open(directory: &Path) -> io::Result<Self> {
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
    pub(super) fn resolve(&self, name_zh: &str, name_en: &str) -> LogoMatch {
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

#[derive(Debug, Snafu)]
pub(crate) enum LogoError {
    #[snafu(display("cannot read image: {source}"))]
    Read { source: io::Error },
    #[snafu(display("expected a regular file, not a symlink or directory"))]
    NotRegularFile,
    #[snafu(display("image exceeds the 8 MiB limit"))]
    TooLarge,
    #[snafu(display("expected PNG, JPEG, WebP or SVG content"))]
    UnsupportedFormat,
    #[snafu(display("image exceeds 4096 pixels per side or 4194304 pixels in total"))]
    Dimensions,
    #[snafu(display("cannot decode image: {source}"))]
    Decode { source: image::ImageError },
    #[snafu(display("cannot parse SVG: {source}"))]
    Svg { source: usvg::Error },
    #[snafu(display("cannot encode PNG: {source}"))]
    Encode { source: io::Error },
    #[snafu(display(
        "SVG images must be embedded, valid PNG, JPEG or WebP within the logo limits; external files and URLs are not loaded"
    ))]
    SvgResources,
    #[snafu(display(
        "SVG text has no usable font; install the required fonts or convert the text to paths"
    ))]
    SvgFont,
}

/// A validated logo, decoded once for HTTP delivery or PNG export.
pub(super) struct DecodedLogo(LogoContent);

enum LogoContent {
    Raster {
        source: Vec<u8>,
        format: ImageFormat,
        image: DynamicImage,
    },
    Svg(tiny_skia::Pixmap),
}

/// The actual image bytes and MIME type, independent of the source extension.
pub(crate) struct LogoImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_type: &'static str,
}

impl DecodedLogo {
    /// Preserve raster source bytes; rasterize SVG for safe browser delivery.
    ///
    /// # Errors
    /// Returns PNG encoding failures for SVG sources.
    pub(super) fn into_http(self) -> Result<LogoImage, LogoError> {
        match self.0 {
            LogoContent::Raster { source, format, .. } => Ok(LogoImage {
                bytes: source,
                content_type: format.to_mime_type(),
            }),
            LogoContent::Svg(image) => Ok(LogoImage {
                bytes: image.encode_png().map_err(|error| LogoError::Encode {
                    source: std::io::Error::other(error),
                })?,
                content_type: "image/png",
            }),
        }
    }

    /// Export a centered 64×64 PNG, preserving aspect ratio and transparency.
    ///
    /// Resamples with Lanczos3 and pads unused canvas space with transparency.
    /// Source files and HTTP delivery are unchanged.
    ///
    /// # Errors
    /// Returns PNG encoding failures.
    pub(super) fn into_domjudge_png(self) -> Result<Vec<u8>, LogoError> {
        const EDGE: u32 = 64;
        let source = match self.0 {
            LogoContent::Raster { image, .. } => image.into_rgba8(),
            LogoContent::Svg(image) => {
                let bytes = image
                    .pixels()
                    .iter()
                    .flat_map(|pixel| {
                        let pixel = pixel.demultiply();
                        [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
                    })
                    .collect();
                image::RgbaImage::from_raw(image.width(), image.height(), bytes)
                    .ok_or(LogoError::Dimensions)?
            }
        };
        // Filter premultiplied alpha so invisible RGB cannot bleed into edges.
        // Reserve headroom for Lanczos's negative lobes: image's float resize
        // clamps to [0, 1], which must not clip alpha before unpremultiplication.
        let premultiplied =
            image::Rgba32FImage::from_fn(source.width(), source.height(), |x, y| {
                let pixel = source.get_pixel(x, y);
                let alpha = f32::from(pixel[3]) / 255.0;
                let encode = |value: f32| 0.25 + 0.25 * value;
                image::Rgba([
                    encode(f32::from(pixel[0]) / 255.0 * alpha),
                    encode(f32::from(pixel[1]) / 255.0 * alpha),
                    encode(f32::from(pixel[2]) / 255.0 * alpha),
                    encode(alpha),
                ])
            });
        let filtered = DynamicImage::ImageRgba32F(premultiplied)
            .resize(EDGE, EDGE, image::imageops::FilterType::Lanczos3)
            .into_rgba32f();
        let straight = image::Rgba32FImage::from_fn(filtered.width(), filtered.height(), |x, y| {
            let pixel = filtered.get_pixel(x, y);
            let decode = |value: f32| (value - 0.25) * 4.0;
            let alpha = decode(pixel[3]);
            let channel = |value: f32| {
                if alpha > 0.0 {
                    (decode(value) / alpha).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
            image::Rgba([
                channel(pixel[0]),
                channel(pixel[1]),
                channel(pixel[2]),
                alpha.clamp(0.0, 1.0),
            ])
        });
        let mut resized = DynamicImage::ImageRgba32F(straight).into_rgba8();
        for pixel in resized.pixels_mut().filter(|pixel| pixel[3] == 0) {
            *pixel = image::Rgba([0, 0, 0, 0]);
        }
        let mut canvas = image::RgbaImage::new(EDGE, EDGE);
        image::imageops::replace(
            &mut canvas,
            &resized,
            i64::from((EDGE - resized.width()) / 2),
            i64::from((EDGE - resized.height()) / 2),
        );
        let mut output = Cursor::new(Vec::new());
        canvas
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|error| LogoError::Encode {
                source: std::io::Error::other(error),
            })?;
        Ok(output.into_inner())
    }
}

/// Read and decode bounded image content without following symlinks.
///
/// # Errors
/// Rejects unreadable/nonregular files, malformed or unsupported content and
/// images exceeding the same byte, dimension and resource limits as preflight.
pub(super) fn read_logo(path: &Path) -> Result<DecodedLogo, LogoError> {
    let mut source = LogoFile::open(path)?;
    decode_logo(&mut source.file)
}

fn decode_logo(file: &mut File) -> Result<DecodedLogo, LogoError> {
    file.rewind().map_err(|source| LogoError::Read { source })?;
    let mut bytes = Vec::new();
    file.take(MAX_LOGO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| LogoError::Read { source })?;
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
    let decoder = reader
        .into_decoder()
        .map_err(|source| LogoError::Decode { source })?;
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height)?;
    DynamicImage::from_decoder(decoder).map_err(|source| LogoError::Decode { source })
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
    // Malformed image input is a domain rejection; text parser detail is irrelevant.
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
    .map_err(|error| LogoError::Svg {
        source: usvg::Error::ParsingFailed(error),
    })?;
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
    let tree = usvg::Tree::from_xmltree(&document, &options)
        .map_err(|source| LogoError::Svg { source })?;
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
