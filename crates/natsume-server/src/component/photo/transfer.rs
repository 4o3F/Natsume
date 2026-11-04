//! A bounded copy, optionally checked against already known image identity.
use super::limits::IO_BUFFER_BYTES;
use natsume_photo_protocol::Sha256Digest;
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::io::{Read, Write};

#[derive(Clone, Copy)]
pub(super) enum Expectation<'a> {
    Upload {
        limit: u64,
    },
    Image {
        bytes: u64,
        sha256: &'a Sha256Digest,
    },
}

#[derive(Debug, Snafu)]
pub(crate) enum TransferError {
    #[snafu(display("Photo input read failed"))]
    Read { source: std::io::Error },
    #[snafu(display("Photo output write failed"))]
    Write { source: std::io::Error },
    #[snafu(display("Photo input exceeds its byte limit"))]
    Limit,
    #[snafu(display("Photo image length or digest disagrees"))]
    Integrity,
    #[snafu(display("Photo copy was interrupted"))]
    Interrupted,
}

/// `copied` remains available after an error so uploads can record their final count.
pub(super) fn copy_verified(
    source: &mut impl Read,
    target: &mut impl Write,
    expected: Expectation<'_>,
    copied: &mut u64,
    mut keep_running: impl FnMut() -> bool,
) -> Result<(), TransferError> {
    let limit = match expected {
        Expectation::Upload { limit } => limit,
        Expectation::Image { bytes, .. } => bytes,
    };
    // Uploads have no expected hash: do not add an unnecessary SHA-256 pass to them.
    let mut hash = matches!(expected, Expectation::Image { .. }).then(Sha256::new);
    let mut buffer = vec![0; IO_BUFFER_BYTES];
    *copied = 0;
    loop {
        ensure!(keep_running(), InterruptedSnafu);
        let count = source.read(&mut buffer).context(ReadSnafu)?;
        if count == 0 {
            break;
        }
        *copied = copied
            .checked_add(count as u64)
            .ok_or_else(|| LimitSnafu.build())?;
        if *copied > limit {
            return match expected {
                Expectation::Upload { .. } => LimitSnafu.fail(),
                Expectation::Image { .. } => IntegritySnafu.fail(),
            };
        }
        if let Some(hash) = &mut hash {
            hash.update(&buffer[..count]);
        }
        target.write_all(&buffer[..count]).context(WriteSnafu)?;
    }
    if let Expectation::Image { bytes, sha256 } = expected {
        ensure!(
            *copied == bytes
                && hash.is_some_and(|hash| hex::encode(hash.finalize()) == sha256.as_str()),
            IntegritySnafu
        );
    }
    Ok(())
}
