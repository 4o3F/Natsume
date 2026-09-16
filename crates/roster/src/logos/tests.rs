use std::{fs, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageFormat};
use tempfile::TempDir;

use super::*;
use crate::parse_xlsx;

fn checked<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("test setup failed: {error:?}"))
}

#[test]
fn opened_logo_keeps_one_descriptor_when_the_path_is_replaced() {
    let root = checked(TempDir::new());
    let path = root.path().join("logo.png");
    write_image(&path, ImageFormat::Png);
    let mut source = checked(LogoFile::open(&path));
    let old = checked(source.fingerprint());
    let replacement = root.path().join("replacement.png");
    checked(fs::write(&replacement, "invalid"));
    checked(fs::rename(replacement, &path));
    checked(source.validate());
    checked(source.validate());
    assert_ne!(checked(checked(LogoFile::open(&path)).fingerprint()), old);
    assert!(validate_logo(&path).is_err());
}

fn write_image(path: &Path, format: ImageFormat) {
    checked(DynamicImage::new_rgb8(8, 8).save_with_format(path, format));
}

#[test]
fn complete_names_match_without_normalizing_punctuation_or_recursing() {
    let roster = checked(parse_xlsx(include_bytes!("../../examples/roster.xlsx")));
    let school = &roster.organizations()[0];
    let directory = checked(TempDir::new());
    let unrelated = directory.path().join("示例大学（校区）.png");
    write_image(&unrelated, ImageFormat::Png);
    checked(fs::create_dir(directory.path().join("nested")));
    write_image(
        &directory.path().join("nested/示例大学.png"),
        ImageFormat::Png,
    );
    assert_eq!(
        checked(LogoDirectory::open(directory.path())).resolve(school.name_zh(), school.name_en()),
        LogoMatch::Missing
    );
    let english = directory.path().join("Example University.WEBP");
    write_image(&english, ImageFormat::WebP);
    assert_eq!(
        checked(LogoDirectory::open(directory.path())).resolve(school.name_zh(), school.name_en()),
        LogoMatch::Unique(english)
    );
}

#[test]
fn multiple_formats_or_languages_for_the_same_school_are_ambiguous() {
    let roster = checked(parse_xlsx(include_bytes!("../../examples/roster.xlsx")));
    let school = &roster.organizations()[0];
    let directory = checked(TempDir::new());
    let first = directory.path().join("示例大学.jpg");
    let second = directory.path().join("示例大学.png");
    let third = directory.path().join("Example University.webp");
    for (path, format) in [
        (&first, ImageFormat::Jpeg),
        (&second, ImageFormat::Png),
        (&third, ImageFormat::WebP),
    ] {
        write_image(path, format);
    }
    let mut expected = vec![first, second, third];
    expected.sort();
    assert_eq!(
        checked(LogoDirectory::open(directory.path())).resolve(school.name_zh(), school.name_en()),
        LogoMatch::Ambiguous(expected)
    );
}

#[test]
fn detects_raster_content_with_webp_extensions_and_leaves_file_bytes_untouched() {
    let directory = checked(TempDir::new());
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
        let path = directory.path().join("logo.webp");
        write_image(&path, format);
        let before = checked(fs::read(&path));
        checked(validate_logo(&path));
        assert_eq!(checked(fs::read(&path)), before);
    }
}

#[test]
fn svg_content_with_xml_header_bom_and_webp_extension_renders_without_mutating_source() {
    let svg = concat!(
        "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!-- logo -->",
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 4 3\">",
        "<rect width=\"2\" height=\"3\" fill=\"#ff0000\"/></svg>"
    );
    let directory = checked(TempDir::new());
    let path = directory.path().join("示例大学.webp");
    checked(fs::write(&path, svg));
    checked(validate_logo(&path));
    assert_eq!(checked(fs::read(&path)), svg.as_bytes());
    let pixels = checked(render_svg(svg.as_bytes()));
    assert_eq!((pixels.width(), pixels.height()), (4, 3));
    assert_eq!(&pixels.data()[..4], [255, 0, 0, 255]);
    assert_eq!(&pixels.data()[12..16], [0, 0, 0, 0]);

    let native = directory.path().join("示例大学.SVG");
    checked(fs::rename(&path, &native));
    let roster = checked(parse_xlsx(include_bytes!("../../examples/roster.xlsx")));
    assert_eq!(
        checked(LogoDirectory::open(directory.path())).resolve(
            roster.organizations()[0].name_zh(),
            roster.organizations()[0].name_en()
        ),
        LogoMatch::Unique(native)
    );
}

#[test]
fn svg_text_is_rendered_or_reports_missing_fonts() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40"><text x="5" y="25" font-size="20">Team</text></svg>"#;
    match render_svg(svg) {
        Ok(pixels) => assert!(pixels.data().chunks_exact(4).any(|rgba| rgba[3] != 0)),
        Err(LogoError::SvgFont) => {}
        Err(error) => panic!("unexpected SVG text error: {error}"),
    }
}

#[test]
fn malformed_svg_and_non_svg_xml_are_rejected_despite_webp_extension() {
    let directory = checked(TempDir::new());
    let path = directory.path().join("logo.webp");
    checked(fs::write(
        &path,
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"><path>",
    ));
    assert!(matches!(validate_logo(&path), Err(LogoError::Svg(_))));
    for input in [
        "<html><body>download failed</body></html>",
        "<root><svg xmlns=\"http://www.w3.org/2000/svg\"/></root>",
        "<svg xmlns=\"urn:wrong-namespace\"/>",
    ] {
        checked(fs::write(&path, input));
        assert!(matches!(
            validate_logo(&path),
            Err(LogoError::UnsupportedFormat)
        ));
    }
}

#[test]
fn svg_dimensions_obey_the_same_pixel_limits_as_raster_logos() {
    for (width, height) in [(4097, 1), (2049, 2048)] {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><rect width=\"1\" height=\"1\"/></svg>"
        );
        assert!(matches!(
            render_svg(svg.as_bytes()),
            Err(LogoError::Dimensions)
        ));
    }
}

#[test]
fn svg_external_images_are_rejected_instead_of_silently_skipped() {
    let directory = checked(TempDir::new());
    let external = directory.path().join("external.png");
    write_image(&external, ImageFormat::Png);
    for href in [
        external.to_string_lossy().as_ref(),
        "https://example.invalid/logo.png",
    ] {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><image href=\"{href}\" width=\"8\" height=\"8\"/></svg>"
        );
        assert!(matches!(
            render_svg(svg.as_bytes()),
            Err(LogoError::SvgResources)
        ));
    }
}

#[test]
fn svg_internal_references_and_embedded_png_are_rendered_and_corrupt_data_is_rejected() {
    let mut png = Cursor::new(Vec::new());
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255]));
    checked(DynamicImage::ImageRgba8(image).write_to(&mut png, ImageFormat::Png));
    let data = STANDARD.encode(png.into_inner());
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"4\" height=\"2\"><defs><rect id=\"r\" width=\"2\" height=\"2\" fill=\"red\"/></defs><use xlink:href=\"#r\"/><image x=\"2\" width=\"2\" height=\"2\" xlink:href=\"data:image/png;base64,{data}\"/></svg>"
    );
    let pixels = checked(render_svg(svg.as_bytes()));
    assert_eq!(&pixels.data()[..4], [255, 0, 0, 255]);
    assert_eq!(&pixels.data()[12..16], [0, 255, 0, 255]);
    let corrupt = svg.replace(&data, &STANDARD.encode(b"broken PNG"));
    assert!(matches!(
        render_svg(corrupt.as_bytes()),
        Err(LogoError::SvgResources)
    ));
}

#[test]
fn missing_directory_and_invalid_image_content_are_distinct() {
    let directory = checked(TempDir::new());
    assert!(LogoDirectory::open(&directory.path().join("absent")).is_err());
    assert!(matches!(
        validate_logo(&directory.path().join("absent.png")),
        Err(LogoError::Read(_))
    ));
    let path = directory.path().join("logo.png");
    checked(fs::write(&path, b"not an image"));
    assert!(matches!(
        validate_logo(&path),
        Err(LogoError::UnsupportedFormat)
    ));
    write_image(&path, ImageFormat::WebP);
    checked(validate_logo(&path));
    checked(fs::write(&path, b"\x89PNG\r\n\x1a\ntruncated"));
    assert!(matches!(validate_logo(&path), Err(LogoError::Decode(_))));
    checked(fs::write(&path, b"GIF89a"));
    assert!(matches!(
        validate_logo(&path),
        Err(LogoError::UnsupportedFormat)
    ));
    let subdirectory = directory.path().join("directory.png");
    checked(fs::create_dir(&subdirectory));
    assert!(matches!(
        validate_logo(&subdirectory),
        Err(LogoError::NotRegularFile)
    ));
}

#[test]
fn compressed_file_and_decoded_pixel_limits_are_enforced() {
    let directory = checked(TempDir::new());
    let path = directory.path().join("logo.png");
    checked(checked(File::create(&path)).set_len(MAX_LOGO_BYTES + 1));
    assert!(matches!(validate_logo(&path), Err(LogoError::TooLarge)));
    checked(DynamicImage::new_rgb8(4097, 1).save_with_format(&path, ImageFormat::Png));
    assert!(matches!(validate_logo(&path), Err(LogoError::Dimensions)));
    checked(DynamicImage::new_rgb8(2049, 2048).save_with_format(&path, ImageFormat::Png));
    assert!(matches!(validate_logo(&path), Err(LogoError::Dimensions)));
}

#[cfg(unix)]
#[test]
fn symlinks_are_reported_without_reading_their_targets() {
    use std::os::unix::fs::symlink;
    let directory = checked(TempDir::new());
    let target = directory.path().join("actual.png");
    write_image(&target, ImageFormat::Png);
    let path = directory.path().join("示例大学.png");
    checked(symlink(target, &path));
    assert!(matches!(
        validate_logo(&path),
        Err(LogoError::NotRegularFile)
    ));
    checked(fs::remove_file(&path));
    checked(symlink(directory.path().join("missing.png"), &path));
    assert!(matches!(
        validate_logo(&path),
        Err(LogoError::NotRegularFile)
    ));
}

#[cfg(unix)]
#[test]
fn unreadable_images_report_the_os_error() {
    use std::os::unix::fs::PermissionsExt;
    let directory = checked(TempDir::new());
    let path = directory.path().join("logo.png");
    write_image(&path, ImageFormat::Png);
    checked(fs::set_permissions(
        &path,
        fs::Permissions::from_mode(0o000),
    ));
    // Privileged test runners may read mode-000 files; ordinary users cannot.
    if File::open(&path).is_err() {
        assert!(
            matches!(validate_logo(&path), Err(LogoError::Read(error)) if error.kind() == io::ErrorKind::PermissionDenied)
        );
    }
}

#[test]
fn png_exports_preserve_decoded_pixels_dimensions_and_alpha_for_every_raster_format() {
    let directory = checked(TempDir::new());
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
        let path = directory.path().join("source.webp");
        let image = if format == ImageFormat::Jpeg {
            DynamicImage::ImageRgb8(image::RgbImage::from_fn(5, 3, |x, y| {
                image::Rgb([
                    checked(u8::try_from(x)) * 40,
                    checked(u8::try_from(y)) * 60,
                    25,
                ])
            }))
        } else {
            DynamicImage::ImageRgba8(image::RgbaImage::from_fn(5, 3, |x, y| {
                image::Rgba([
                    checked(u8::try_from(x)) * 40,
                    checked(u8::try_from(y)) * 60,
                    25,
                    checked(u8::try_from(x)) * 50,
                ])
            }))
        };
        checked(image.save_with_format(&path, format));
        let before = checked(fs::read(&path));
        let expected = checked(image::load_from_memory(&before)).to_rgba8();
        let png = checked(checked(read_logo(&path)).into_png());
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(checked(image::load_from_memory(&png)).to_rgba8(), expected);
        let http = checked(checked(read_logo(&path)).into_http());
        assert_eq!(http.content_type, format.to_mime_type());
        assert_eq!(http.bytes, before);
        assert_eq!(checked(fs::read(&path)), before);
    }
}

#[test]
fn svg_http_and_export_share_natural_canvas_transparent_pixels_and_png_encoding() {
    let directory = checked(TempDir::new());
    let path = directory.path().join("svg-disguised.webp");
    let source = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"><rect width="4" height="4" fill="#ff0000" fill-opacity="0.5"/></svg>"##;
    checked(fs::write(&path, source));
    let png = checked(checked(read_logo(&path)).into_png());
    let http = checked(checked(read_logo(&path)).into_http());
    assert_eq!(http.bytes, png);
    assert_eq!(http.content_type, "image/png");
    let image = checked(image::load_from_memory(&png)).to_rgba8();
    assert_eq!(image.dimensions(), (8, 4));
    assert_eq!(image.get_pixel(1, 1).0, [255, 0, 0, 128]);
    assert_eq!(image.get_pixel(6, 1).0, [0, 0, 0, 0]);
    assert_eq!(checked(fs::read(&path)), source);
}
