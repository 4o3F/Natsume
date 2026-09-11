use std::{
    fs::{self, File, Metadata},
    io::Read as _,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use natsume_local_control_api::ResourceControlError;
use procfs::{FromRead as _, process::MountInfos};

use super::{rejected, unavailable};

const BASE: &str = "usr/lib/natsume/home-templates";

fn trusted_metadata(path: &Path, directory: bool) -> Result<Metadata, ResourceControlError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| unavailable("Home template metadata is unavailable"))?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(rejected("Home template metadata is not root controlled"));
    }
    Ok(metadata)
}

fn image_path(root: &Path) -> Result<PathBuf, ResourceControlError> {
    let base = root.join(BASE);
    trusted_metadata(&base, true)?;
    trusted_metadata(&base.join("current"), true)?;
    let marker = base.join("current/version");
    trusted_metadata(&marker, false)?;
    let mut version = String::new();
    File::open(marker)
        .and_then(|file| file.take(74).read_to_string(&mut version))
        .map_err(|_| unavailable("Home template version is unreadable"))?;
    let version = version.strip_suffix('\n').unwrap_or(&version);
    let digest = version.strip_prefix("sha256-");
    if !digest.is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(rejected("Home template version is invalid"));
    }
    let directory = base.join(version);
    trusted_metadata(&directory, true)?;
    let image = directory.join("home.squashfs");
    if trusted_metadata(&image, false)?.len() == 0 {
        return Err(rejected("Home template image is empty"));
    }
    Ok(image)
}

/// Checks the actual immutable lower and its selected, root-controlled source.
/// The image builder verifies the content digest; this frequent observation
/// checks kernel mount/loop facts without rehashing an entire image on each tick.
pub(super) fn verify(root: &Path, home: &Metadata) -> Result<(), ResourceControlError> {
    let image = image_path(root)?;
    let lower = root.join(super::TEMPLATE_RELATIVE_PATH);
    let metadata = fs::symlink_metadata(&lower)
        .map_err(|_| unavailable("Home template mount is unavailable"))?;
    if !metadata.is_dir() || metadata.uid() != home.uid() || metadata.gid() != home.gid() {
        return Err(rejected(
            "Home template ownership differs from contest Home",
        ));
    }
    let mounts = MountInfos::from_file(root.join("proc/self/mountinfo"))
        .map_err(|_| unavailable("Home template mount state is unavailable"))?;
    let mut at_lower = mounts
        .iter()
        .filter(|mount| mount.mount_point.starts_with(&lower));
    let Some(mount) = at_lower.next() else {
        return Err(unavailable("Home template is not mounted read-only"));
    };
    if at_lower.next().is_some()
        || mount.mount_point != lower
        || mount.root != "/"
        || mount.fs_type != "squashfs"
        || !mount.mount_options.contains_key("ro")
        || !mount.super_options.contains_key("ro")
    {
        return Err(rejected("Home template is not a unique read-only SquashFS"));
    }
    let backing = fs::read_to_string(
        root.join("sys/dev/block")
            .join(&mount.majmin)
            .join("loop/backing_file"),
    )
    .map_err(|_| unavailable("Home template loop source is unavailable"))?;
    if Path::new(backing.trim_end_matches('\n'))
        != Path::new("/").join(
            image
                .strip_prefix(root)
                .map_err(|_| rejected("Home template image is outside its fixed root"))?,
        )
    {
        return Err(rejected("Home template mount uses a different version"));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use std::{error::Error, os::unix::fs::PermissionsExt as _};

    use super::*;

    pub(in crate::home) fn fixture(root: &Path) -> Result<(), Box<dyn Error>> {
        let version = format!("sha256-{}", "a".repeat(64));
        let base = root.join(BASE);
        fs::create_dir_all(base.join("current/lower"))?;
        fs::create_dir_all(base.join(&version))?;
        fs::write(base.join("current/version"), format!("{version}\n"))?;
        fs::write(base.join(&version).join("home.squashfs"), "fixture image")?;
        fs::create_dir_all(root.join("proc/self"))?;
        fs::write(
            root.join("proc/self/mountinfo"),
            format!(
                "42 25 7:3 / {} ro,nodev,nosuid - squashfs /dev/loop3 ro\n",
                base.join("current/lower").display()
            ),
        )?;
        fs::create_dir_all(root.join("sys/dev/block/7:3/loop"))?;
        fs::write(
            root.join("sys/dev/block/7:3/loop/backing_file"),
            format!("/{BASE}/{version}/home.squashfs\n"),
        )?;
        Ok(())
    }

    #[test]
    fn only_the_selected_read_only_mount_is_a_valid_lower() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        fixture(root.path())?;
        let lower = root.path().join(super::super::TEMPLATE_RELATIVE_PATH);
        let home = fs::metadata(&lower)?;
        verify(root.path(), &home)?;
        let mountinfo = root.path().join("proc/self/mountinfo");
        let valid = fs::read_to_string(&mountinfo)?;
        for invalid in [
            String::new(),
            valid.replace("ro,nodev", "rw,nodev"),
            valid.replace("/dev/loop3 ro", "/dev/loop3 rw"),
            valid.replace("squashfs", "ext4"),
            valid.replace("7:3 / ", "7:3 /subtree "),
            format!("{valid}{valid}"),
            format!(
                "{valid}43 42 0:4 / {}/.config rw - tmpfs tmpfs rw\n",
                lower.display()
            ),
        ] {
            fs::write(&mountinfo, invalid)?;
            assert!(verify(root.path(), &home).is_err());
        }
        fs::write(&mountinfo, valid)?;
        fs::write(
            root.path().join("sys/dev/block/7:3/loop/backing_file"),
            "/other/home.squashfs\n",
        )?;
        assert!(verify(root.path(), &home).is_err());
        Ok(())
    }

    #[test]
    fn version_and_source_cannot_escape_the_root_owned_contract() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        fixture(root.path())?;
        let marker = root.path().join(BASE).join("current/version");
        let valid = fs::read_to_string(&marker)?;
        let image = image_path(root.path())?;
        for invalid in ["../other", "sha256-deadbeef", "sha256-", "", "v1\nv2\n"] {
            fs::write(&marker, invalid)?;
            assert!(image_path(root.path()).is_err());
        }
        fs::write(&marker, valid)?;
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o666))?;
        assert!(image_path(root.path()).is_err());
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o644))?;
        fs::remove_file(&image)?;
        assert!(image_path(root.path()).is_err());
        std::os::unix::fs::symlink(&marker, &image)?;
        assert!(image_path(root.path()).is_err());
        Ok(())
    }
}
