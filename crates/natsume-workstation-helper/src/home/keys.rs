//! A successful reset proves that old UID-owned keys have left the kernel's
//! serial inventory, including keys retained only by keyring restrictions.
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read as _,
    path::Path,
    time::{Duration, Instant},
};

use keyutils_raw::{KeyringSerial, keyctl_describe, keyctl_invalidate, keyctl_link};
use snafu::{ResultExt as _, Snafu, ensure};

const PROC_LIMIT: u64 = 8 * 1024 * 1024;
const DESCRIPTION_LIMIT: usize = 8192;

#[derive(Debug, Snafu)]
pub(super) enum Error {
    #[snafu(display("contestant key inventory is unavailable"))]
    Io { source: std::io::Error },
    #[snafu(display("contestant key operation was denied or failed"))]
    Operation { source: keyutils::Error },
    #[snafu(display("contestant key inventory cannot prove complete cleanup"))]
    Inventory,
    #[snafu(display("contestant key cleanup deadline expired"))]
    Deadline,
}

type Result<T> = std::result::Result<T, Error>;

fn before(deadline: Instant) -> Result<()> {
    ensure!(Instant::now() < deadline, DeadlineSnafu);
    Ok(())
}

fn proc_file(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)
        .context(IoSnafu)?
        .take(PROC_LIMIT + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(bytes.len() as u64 <= PROC_LIMIT, InventorySnafu);
    Ok(bytes)
}

fn decimal(bytes: &[u8]) -> Result<u32> {
    ensure!(
        !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit),
        InventorySnafu
    );
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| InventorySnafu.build())
}

fn uid_total(bytes: &[u8], uid: u32) -> Result<usize> {
    let mut total = None;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let fields: Vec<_> = line
            .split(u8::is_ascii_whitespace)
            .filter(|field| !field.is_empty())
            .collect();
        ensure!(fields.len() == 5, InventorySnafu);
        let owner = fields[0]
            .strip_suffix(b":")
            .ok_or_else(|| InventorySnafu.build())?;
        if decimal(owner)? != uid {
            continue;
        }
        ensure!(total.is_none(), InventorySnafu);
        let mut counts = fields[2].split(|byte| *byte == b'/');
        let count = decimal(counts.next().ok_or_else(|| InventorySnafu.build())?)?;
        let instantiated = decimal(counts.next().ok_or_else(|| InventorySnafu.build())?)?;
        ensure!(
            counts.next().is_none() && instantiated <= count,
            InventorySnafu
        );
        total = Some(count as usize);
    }
    total.ok_or_else(|| InventorySnafu.build())
}

// Descriptions can contain newlines and counterfeit /proc/keys rows. These are
// only candidate serials: never trust their printed UID, flags or description.
// Every distinct candidate is authenticated with kernel KEYCTL_DESCRIBE below.
fn candidates(bytes: &[u8]) -> Result<BTreeSet<KeyringSerial>> {
    let mut ids = BTreeSet::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        ensure!(
            line.len() > 8
                && line[8].is_ascii_whitespace()
                && line[..8].iter().all(u8::is_ascii_hexdigit),
            InventorySnafu
        );
        let serial = std::str::from_utf8(&line[..8])
            .ok()
            .and_then(|value| i32::from_str_radix(value, 16).ok())
            .and_then(KeyringSerial::new)
            .filter(|serial| serial.get() > 0)
            .ok_or_else(|| InventorySnafu.build())?;
        ids.insert(serial);
    }
    Ok(ids)
}

fn described_uid(bytes: &[u8]) -> Result<u32> {
    ensure!(bytes.last() == Some(&0), InventorySnafu);
    let mut fields = bytes[..bytes.len() - 1].splitn(5, |byte| *byte == b';');
    let kind = fields.next().ok_or_else(|| InventorySnafu.build())?;
    ensure!(
        !kind.is_empty() && kind.iter().all(u8::is_ascii_graphic),
        InventorySnafu
    );
    let uid = decimal(fields.next().ok_or_else(|| InventorySnafu.build())?)?;
    decimal(fields.next().ok_or_else(|| InventorySnafu.build())?)?;
    let permissions = fields.next().ok_or_else(|| InventorySnafu.build())?;
    ensure!(
        permissions.len() == 8
            && permissions.iter().all(u8::is_ascii_hexdigit)
            && fields.next().is_some(),
        InventorySnafu
    );
    Ok(uid)
}

fn owner(id: KeyringSerial) -> Result<Option<u32>> {
    let size = match keyctl_describe(id, None) {
        Ok(size) => size,
        Err(error) if error.0 == rustix::io::Errno::NOKEY.raw_os_error() => return Ok(None),
        Err(source) => return Err(source).context(OperationSnafu),
    };
    ensure!(size > 0 && size <= DESCRIPTION_LIMIT, InventorySnafu);
    let mut bytes = vec![0; size];
    let actual = keyctl_describe(id, Some(bytes.as_mut_slice().into())).context(OperationSnafu)?;
    ensure!(actual == size, InventorySnafu);
    described_uid(&bytes).map(Some)
}

fn verified_owned(
    ids: &BTreeSet<KeyringSerial>,
    uid: u32,
    mut describe: impl FnMut(KeyringSerial) -> Result<Option<u32>>,
) -> Result<BTreeSet<KeyringSerial>> {
    let mut owned = BTreeSet::new();
    for id in ids {
        if describe(*id)? == Some(uid) {
            owned.insert(*id);
        }
    }
    Ok(owned)
}

fn inventory(root: &Path, uid: u32, deadline: Instant) -> Result<BTreeSet<KeyringSerial>> {
    loop {
        before(deadline)?;
        let before_count = uid_total(&proc_file(&root.join("proc/key-users"))?, uid)?;
        let ids = candidates(&proc_file(&root.join("proc/keys"))?)?;
        let owned = verified_owned(&ids, uid, owner)?;
        let after_count = uid_total(&proc_file(&root.join("proc/key-users"))?, uid)?;
        if before_count == after_count && owned.len() == after_count {
            return Ok(owned);
        }
        // A hidden READ-only key must not be mistaken for an empty ring. GC
        // races may settle; an inventory gap never becomes successful cleanup.
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn retired(
    ids: &BTreeSet<KeyringSerial>,
    old: &BTreeSet<KeyringSerial>,
    total: usize,
    session: KeyringSerial,
) -> bool {
    total == 1 && ids.contains(&session) && ids.is_disjoint(old)
}

fn wait_retired(
    root: &Path,
    uid: u32,
    session: KeyringSerial,
    old: &BTreeSet<KeyringSerial>,
    deadline: Instant,
) -> Result<()> {
    loop {
        before(deadline)?;
        let count = uid_total(&proc_file(&root.join("proc/key-users"))?, uid)?;
        let ids = candidates(&proc_file(&root.join("proc/keys"))?)?;
        if retired(&ids, old, count, session)
            && owner(session)? == Some(uid)
            && old
                .iter()
                .map(|id| owner(*id))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .all(Option::is_none)
            && uid_total(&proc_file(&root.join("proc/key-users"))?, uid)? == 1
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn reset(uid: u32) -> Result<()> {
    use keyutils_raw::{
        KEY_SPEC_USER_KEYRING, KEY_SPEC_USER_SESSION_KEYRING, keyctl_clear, keyctl_get_keyring_id,
        keyctl_get_persistent, keyctl_join_session_keyring,
    };
    let root = Path::new("/");
    let deadline = Instant::now() + Duration::from_secs(1);
    let session = keyctl_join_session_keyring(None).context(OperationSnafu)?;
    let user = keyctl_get_keyring_id(KEY_SPEC_USER_KEYRING, true).context(OperationSnafu)?;
    let user_session =
        keyctl_get_keyring_id(KEY_SPEC_USER_SESSION_KEYRING, true).context(OperationSnafu)?;
    keyctl_link(user, session).context(OperationSnafu)?;
    keyctl_link(user_session, session).context(OperationSnafu)?;
    keyctl_get_persistent(uid, session).context(OperationSnafu)?;
    let mut old = inventory(root, uid, deadline)?;
    ensure!(old.remove(&session), InventorySnafu);
    // Pin every old owned key directly before invalidating anything. Ordinary
    // link traversal misses restriction-only references; direct possession also
    // survives invalidating parent rings in any serial order.
    for id in &old {
        before(deadline)?;
        keyctl_link(*id, session).context(OperationSnafu)?;
    }
    for id in &old {
        before(deadline)?;
        keyctl_invalidate(*id).context(OperationSnafu)?;
    }
    keyctl_clear(session).context(OperationSnafu)?;
    // Invalidation alone leaves descriptions visible in /proc/keys until GC.
    // Require their actual removal and account for invisible keys too, without
    // waiting for the normal 300-second revoked-key GC delay or reading payloads.
    wait_retired(root, uid, session, &old, deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: i32) -> KeyringSerial {
        KeyringSerial::new(value).unwrap_or_else(|| panic!("nonzero fixture ID"))
    }

    #[test]
    fn describe_uid_comes_from_kernel_prefix_not_description_fields() {
        assert_eq!(described_uid(b"user;2000;3000;003f0000;secret;0;0;00000000;fake\n00000002 I--Q--- 1 perm ffffffff 0 0 user fake\0").ok(), Some(2000));
        for invalid in [
            b"user;2000;3000;003f0000;missing nul".as_slice(),
            b"user;fake;3000;003f0000;x\0",
            b"user;2000;3000;bad;x\0",
            b"user;2000;3000;003f0000\0",
        ] {
            assert!(described_uid(invalid).is_err());
        }
    }

    #[test]
    fn forged_proc_rows_cannot_replace_hidden_uid_keys_or_duplicate_counts() {
        let rows = b"00000001 I--Q--- 1 perm 003f0000 2000 3000 user real\n00000001 I--Q--- 1 perm 003f0000 2000 3000 user duplicate\n00000002 I--Q--- 1 perm 003f0000 2000 3000 user forged-owner\n";
        let ids = candidates(rows).unwrap_or_else(|error| panic!("fixture candidates: {error}"));
        let owned = verified_owned(&ids, 2000, |serial| {
            Ok(Some(if serial == id(1) { 2000 } else { 2001 }))
        })
        .unwrap_or_else(|error| panic!("fixture owners: {error}"));
        assert_eq!(owned, BTreeSet::from([id(1)]));
        assert_ne!(owned.len(), 2); // Hidden key cannot be covered by forged UID text.
        assert!(candidates(b"00000001 valid\nnot a kernel row\n").is_err());
        assert!(candidates(b"00000000 invalid serial\n").is_err());
        assert!(verified_owned(&ids, 2000, |_| InventorySnafu.fail()).is_err());
    }

    #[test]
    fn uid_counters_require_unique_complete_numeric_row() {
        let bytes = b"    0: 5 4/4 3/1000000 10/1000000\n 2000: 7 6/5 4/200 30/20000\n";
        assert_eq!(uid_total(bytes, 2000).ok(), Some(6));
        for invalid in [
            b"2000: 1 2/3 1/200 1/20000\n".as_slice(),
            b"2000: 1 2 1/200 1/20000\n",
            b"2000: 1 2/2 1/200 1/20000\n2000: 1 2/2 1/200 1/20000\n",
            b"2001: 1 2/2 1/200 1/20000\n",
        ] {
            assert!(uid_total(invalid, 2000).is_err());
        }
    }

    #[test]
    fn invalidated_keys_and_metadata_must_leave_inventory_before_success() {
        let session = id(1);
        let old = BTreeSet::from([id(2), id(3)]); // restriction-pinned ring and payload
        assert!(!retired(
            &BTreeSet::from([session, id(2)]),
            &old,
            2,
            session
        ));
        assert!(!retired(&BTreeSet::from([session]), &old, 2, session)); // invisible old key
        assert!(!retired(
            &BTreeSet::from([session, id(2)]),
            &old,
            1,
            session
        )); // lingering metadata
        assert!(retired(&BTreeSet::from([session]), &old, 1, session));
    }
}
