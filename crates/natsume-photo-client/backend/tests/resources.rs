use std::{path::PathBuf, process::Command};

use natsume_photo_client::{
    roster::Roster,
    workspace::{Error as WorkspaceError, Workspace},
};
use serde_json::{Value, json};
use snafu::{ResultExt as _, Snafu};

#[derive(Debug, Snafu)]
enum Error {
    Io {
        source: std::io::Error,
    },
    Image {
        source: image::ImageError,
    },
    Json {
        source: serde_json::Error,
    },
    Roster {
        source: natsume_photo_client::roster::Error,
    },
    Workspace {
        source: WorkspaceError,
    },
}
type Result<T = ()> = std::result::Result<T, Error>;

struct Fixture {
    directory: tempfile::TempDir,
    roster: Roster,
    manifest: PathBuf,
}
impl Fixture {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir().context(IoSnafu)?;
        let source = directory.path().join("input");
        std::fs::create_dir(&source).context(IoSnafu)?;
        image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 40]))
            .save(source.join("logo.png"))
            .context(ImageSnafu)?;
        let mut rows: Value =
            serde_json::from_slice(include_bytes!("fixtures/roster.json")).context(JsonSnafu)?;
        rows[1]["organization_id"] = json!(1);
        let roster =
            Roster::parse(&serde_json::to_vec(&rows).context(JsonSnafu)?).context(RosterSnafu)?;
        let fixture = Self {
            directory,
            roster,
            manifest: source.join("resources.json"),
        };
        fixture.write(&json!({"school_logos": [{"organization_id": 1, "path": "logo.png"}], "default_image": "logo.png"}))?;
        Ok(fixture)
    }
    fn write(&self, input: &Value) -> Result {
        std::fs::write(
            &self.manifest,
            serde_json::to_vec(input).context(JsonSnafu)?,
        )
        .context(IoSnafu)
    }
    fn root(&self) -> PathBuf {
        self.directory.path().join("工作区 #资源%")
    }
}

#[test]
fn shared_school_logo_and_default_survive_source_removal_and_reopen() -> Result {
    let fixture = Fixture::new()?;
    let mut workspace =
        Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest)
            .context(WorkspaceSnafu)?;
    let first = &fixture.roster.teams()[0].account_id;
    let second = &fixture.roster.teams()[1].account_id;
    let logo = workspace.logo_path(first).context(WorkspaceSnafu)?;
    assert!(logo.is_some());
    assert_eq!(logo, workspace.logo_path(second).context(WorkspaceSnafu)?);
    assert_eq!(
        logo,
        workspace.default_image_path().context(WorkspaceSnafu)?
    );
    assert_eq!(
        std::fs::read_dir(fixture.root().join("resources"))
            .context(IoSnafu)?
            .count(),
        1
    );
    drop(workspace);
    std::fs::remove_file(fixture.directory.path().join("input/logo.png")).context(IoSnafu)?;
    std::fs::remove_file(&fixture.manifest).context(IoSnafu)?;
    let mut reopened = Workspace::open(&fixture.root()).context(WorkspaceSnafu)?;
    assert_eq!(reopened.logo_path(first).context(WorkspaceSnafu)?, logo);
    assert_eq!(reopened.default_image_path().context(WorkspaceSnafu)?, logo);
    Ok(())
}

#[test]
fn absent_resources_are_optional_and_unknown_team_is_distinct() -> Result {
    let fixture = Fixture::new()?;
    let mut workspace =
        Workspace::create(&fixture.root(), &fixture.roster).context(WorkspaceSnafu)?;
    assert!(
        workspace
            .logo_path(&fixture.roster.teams()[0].account_id)
            .context(WorkspaceSnafu)?
            .is_none()
    );
    assert!(
        workspace
            .default_image_path()
            .context(WorkspaceSnafu)?
            .is_none()
    );
    assert!(matches!(
        workspace.logo_path("unknown"),
        Err(WorkspaceError::UnknownTeam)
    ));
    Ok(())
}

#[test]
fn valid_but_changed_cached_image_is_rejected_before_and_after_reopen() -> Result {
    let fixture = Fixture::new()?;
    let mut workspace =
        Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest)
            .context(WorkspaceSnafu)?;
    let logo = workspace
        .logo_path(&fixture.roster.teams()[0].account_id)
        .context(WorkspaceSnafu)?;
    assert!(logo.is_some());
    if let Some(path) = logo {
        image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
            .save(path)
            .context(ImageSnafu)?;
    }
    assert!(matches!(
        workspace.default_image_path(),
        Err(WorkspaceError::Resource)
    ));
    drop(workspace);
    assert!(matches!(
        Workspace::open(&fixture.root()),
        Err(WorkspaceError::Resource)
    ));
    Ok(())
}

#[test]
fn missing_cached_resource_is_not_silently_treated_as_unconfigured() -> Result {
    let fixture = Fixture::new()?;
    let mut workspace =
        Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest)
            .context(WorkspaceSnafu)?;
    if let Some(path) = workspace.default_image_path().context(WorkspaceSnafu)? {
        std::fs::remove_file(path).context(IoSnafu)?;
    }
    drop(workspace);
    assert!(matches!(
        Workspace::open(&fixture.root()),
        Err(WorkspaceError::Io { .. })
    ));
    Ok(())
}

#[test]
fn escaping_or_absolute_source_paths_are_rejected_without_target_creation() -> Result {
    let fixture = Fixture::new()?;
    for relative in ["../logo.png", "/outside.png", "C:\\outside.png"] {
        fixture.write(&json!({"default_image": relative}))?;
        assert!(matches!(
            Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest),
            Err(WorkspaceError::Path)
        ));
        assert!(!fixture.root().exists());
    }
    Ok(())
}

#[test]
fn duplicate_or_unknown_school_mappings_are_rejected() -> Result {
    let fixture = Fixture::new()?;
    for logos in [
        json!([{"organization_id": 999, "path": "logo.png"}]),
        json!([{"organization_id": 1, "path": "logo.png"}, {"organization_id": 1, "path": "logo.png"}]),
    ] {
        fixture.write(&json!({"school_logos": logos}))?;
        assert!(matches!(
            Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest),
            Err(WorkspaceError::Resource)
        ));
        assert!(!fixture.root().exists());
    }
    Ok(())
}

#[test]
fn missing_or_fake_images_fail_without_creating_workspace() -> Result {
    let fixture = Fixture::new()?;
    fixture.write(&json!({"default_image": "missing.png"}))?;
    assert!(matches!(
        Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest),
        Err(WorkspaceError::Io { .. })
    ));
    std::fs::write(
        fixture.directory.path().join("input/logo.png"),
        b"not an image",
    )
    .context(IoSnafu)?;
    fixture.write(&json!({"default_image": "logo.png"}))?;
    assert!(
        Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest)
            .is_err()
    );
    assert!(!fixture.root().exists());
    Ok(())
}

#[test]
fn jpeg_is_cached_with_its_real_extension() -> Result {
    let fixture = Fixture::new()?;
    image::RgbImage::from_pixel(4, 4, image::Rgb([3, 4, 5]))
        .save(fixture.directory.path().join("input/logo.jpg"))
        .context(ImageSnafu)?;
    fixture.write(&json!({"default_image": "logo.jpg"}))?;
    let mut workspace =
        Workspace::create_with_resources(&fixture.root(), &fixture.roster, &fixture.manifest)
            .context(WorkspaceSnafu)?;
    let path = workspace.default_image_path().context(WorkspaceSnafu)?;
    assert!(path.is_some_and(|path| path.extension().is_some_and(|extension| extension == "jpg")));
    Ok(())
}

#[test]
fn cli_resource_creation_survives_a_separate_inspection_process() -> Result {
    let fixture = Fixture::new()?;
    let roster_file = fixture.directory.path().join("roster.json");
    std::fs::write(&roster_file, include_bytes!("fixtures/roster.json")).context(IoSnafu)?;
    let created = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("create-workspace")
        .arg(fixture.root())
        .arg("--roster")
        .arg(roster_file)
        .arg("--resources")
        .arg(&fixture.manifest)
        .output()
        .context(IoSnafu)?;
    assert!(created.status.success());
    let inspected = Command::new(env!("CARGO_BIN_EXE_natsume-photo-client"))
        .arg("inspect-workspace")
        .arg(fixture.root())
        .output()
        .context(IoSnafu)?;
    assert!(inspected.status.success());
    Ok(())
}
