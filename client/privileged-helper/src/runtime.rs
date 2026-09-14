//! Small, root-owned observations shared by the service and its fixed login unit.

use std::{
    fs::{self, File},
    io::{Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use natsume_local_control_api::ResourceControlError;
use rustix::fs::{Mode, OFlags};
use serde::{Serialize, de::DeserializeOwned};

use crate::session::{rejected, unavailable};

const MAX_RECORD_BYTES: u64 = 65_536;

fn directory(root: &Path) -> Result<PathBuf, ResourceControlError> {
    let path = root.join("run/natsume-privileged");
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| unavailable("graphical runtime directory is unavailable"))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(rejected("graphical runtime directory is unsafe"));
    }
    Ok(path)
}

fn checked_file(metadata: &fs::Metadata) -> Result<(), ResourceControlError> {
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(rejected("graphical runtime record is unsafe"));
    }
    Ok(())
}

pub(crate) fn read<T: DeserializeOwned>(
    root: &Path,
    name: &'static str,
) -> Result<Option<T>, ResourceControlError> {
    let path = directory(root)?.join(name);
    let fd = match rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(unavailable("graphical runtime record cannot be opened")),
    };
    let file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|_| unavailable("graphical runtime record cannot be inspected"))?;
    checked_file(&metadata)?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable("graphical runtime record cannot be read"))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(rejected("graphical runtime record exceeds its size limit"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| rejected("graphical runtime record is invalid"))
}

/// Each record has one writer: the registration observer or the mutation owner.
pub(crate) fn write<T: Serialize>(
    root: &Path,
    name: &'static str,
    value: &T,
) -> Result<(), ResourceControlError> {
    let directory = directory(root)?;
    let path = directory.join(name);
    let temporary = path.with_extension("pending");
    if let Ok(metadata) = fs::symlink_metadata(&temporary) {
        checked_file(&metadata)?;
        fs::remove_file(&temporary)
            .map_err(|_| unavailable("graphical runtime temporary record cannot be removed"))?;
    }
    let bytes = serde_json::to_vec(value)
        .map_err(|_| unavailable("graphical runtime record cannot be encoded"))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(rejected("graphical runtime record exceeds its size limit"));
    }
    let mut file = File::from(
        rustix::fs::open(
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| unavailable("graphical runtime record cannot be prepared"))?,
    );
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temporary, &path))
        .and_then(|()| File::open(&directory)?.sync_all())
        .map_err(|_| unavailable("graphical runtime record cannot be published"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    #[test]
    fn shared_records_are_private_and_refuse_links() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("run/natsume-privileged");
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        write(root.path(), "test.json", &vec![1_u32, 2])?;
        assert_eq!(
            read::<Vec<u32>>(root.path(), "test.json")?,
            Some(vec![1, 2])
        );
        assert_eq!(
            fs::metadata(directory.join("test.json"))?.mode() & 0o777,
            0o600
        );
        symlink(directory.join("test.json"), directory.join("linked.json"))?;
        assert!(read::<Vec<u32>>(root.path(), "linked.json").is_err());
        fs::set_permissions(
            directory.join("test.json"),
            fs::Permissions::from_mode(0o644),
        )?;
        assert!(read::<Vec<u32>>(root.path(), "test.json").is_err());
        Ok(())
    }

    #[test]
    fn malformed_and_oversized_records_never_become_observations()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("run/natsume-privileged");
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        write(root.path(), "test.json", &1_u32)?;
        fs::write(directory.join("test.json"), b"{")?;
        assert!(read::<u32>(root.path(), "test.json").is_err());
        fs::write(directory.join("test.json"), vec![b' '; 65_537])?;
        assert!(read::<u32>(root.path(), "test.json").is_err());
        Ok(())
    }
}
