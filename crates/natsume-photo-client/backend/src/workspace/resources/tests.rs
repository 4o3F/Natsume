use super::*;
use crate::{
    roster::Roster,
    workspace::{Error, Workspace},
};

#[derive(Debug, snafu::Snafu)]
enum TestError {
    Io { source: std::io::Error },
    Image { source: image::ImageError },
    Resource { source: ResourceError },
    Roster { source: crate::roster::Error },
}
type TestResult<T = ()> = std::result::Result<T, TestError>;

#[tokio::test]
async fn changed_source_during_preparation_leaves_workspace_incomplete() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let logo = directory.path().join("logo.png");
    image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
        .save(&logo)
        .context(ImageSnafu)?;
    let manifest = directory.path().join("resources.json");
    std::fs::write(
        &manifest,
        br#"{"school_logos":[{"organization_id":1,"path":"logo.png"}]}"#,
    )
    .context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../../../tests/fixtures/roster.json"))
        .context(RosterSnafu)?;
    let prepared = Prepared::load(
        &manifest,
        &roster.teams().iter().map(|t| t.organization_id).collect(),
    )
    .context(ResourceSnafu)?;
    image::RgbImage::from_pixel(4, 4, image::Rgb([4, 5, 6]))
        .save(&logo)
        .context(ImageSnafu)?;
    let root = directory.path().join("workspace");
    assert!(matches!(
        Workspace::create_prepared(&root, &roster, Some(prepared)).await,
        Err(Error::Resource { .. })
    ));
    assert!(matches!(
        Workspace::open(&root).await,
        Err(Error::Incomplete)
    ));
    Ok(())
}

#[test]
fn byte_and_pixel_limits_reject_resources() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("large.png");
    let file = std::fs::File::create(&path).context(IoSnafu)?;
    file.set_len(16 * 1024 * 1024 + 1).context(IoSnafu)?;
    drop(file);
    assert!(matches!(read_image(&path), Err(ResourceError::ByteLimit)));
    image::RgbImage::from_pixel(4097, 1, image::Rgb([1, 2, 3]))
        .save(&path)
        .context(ImageSnafu)?;
    assert!(matches!(
        read_image(&path),
        Err(ResourceError::Decode { .. })
    ));
    Ok(())
}

#[test]
fn persisted_path_cannot_escape_content_addressed_resource_directory() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let reference = ImageRef {
        path: "../outside.png".into(),
        sha256: "0".repeat(64),
    };
    assert!(matches!(
        reference.verify(directory.path()),
        Err(ResourceError::Path)
    ));
    Ok(())
}

#[test]
fn manifest_rejects_unknown_fields() -> TestResult {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("resources.json");
    std::fs::write(&path, br#"{"school_logos":[],"password":"forbidden"}"#).context(IoSnafu)?;
    let roster = Roster::parse(include_bytes!("../../../tests/fixtures/roster.json"))
        .context(RosterSnafu)?;
    assert!(matches!(
        Prepared::load(
            &path,
            &roster.teams().iter().map(|t| t.organization_id).collect()
        ),
        Err(ResourceError::Manifest { .. })
    ));
    Ok(())
}
