//! Decode verified candidate bytes off the UI thread. Never let Slint reopen a camera path.
use natsume_photo_client::capture_session::CandidateImage;
use sha2::{Digest as _, Sha256};
use snafu::{ResultExt as _, Snafu, ensure};
use std::{io::Read as _, sync::Arc};

#[derive(Debug, Snafu)]
pub(super) enum Error {
    #[snafu(display("preview image could not be opened"))]
    Candidate {
        source: natsume_photo_client::capture_session::CandidateError,
    },
    #[snafu(display("preview image could not be read"))]
    Io { source: std::io::Error },
    #[snafu(display("preview image failed integrity validation"))]
    Integrity,
    #[snafu(display("preview image could not be decoded"))]
    Decode { source: image::ImageError },
}

#[derive(Debug)]
pub(super) struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    pub fn image(&self) -> slint::Image {
        slint::Image::from_rgba8(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &self.rgba,
                self.width,
                self.height,
            ),
        )
    }
}

pub(super) fn candidate(image: &CandidateImage) -> Result<Arc<Pixels>, Error> {
    // The handle denies writes and deletion until verification AND decoding finish.
    let file = image.open().context(CandidateSnafu)?;
    verified(file, image.bytes(), image.sha256(), image.dimensions())
}

pub(super) fn confirmed(
    root: &std::path::Path,
    photo: &natsume_photo_client::workspace::ConfirmedPhoto,
) -> Result<Arc<Pixels>, Error> {
    use std::os::windows::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(root.join(photo.path()))
        .context(IoSnafu)?;
    verified(file, photo.bytes(), photo.sha256(), photo.dimensions())
}

fn verified(
    mut file: std::fs::File,
    length: u64,
    sha256: &str,
    dimensions: natsume_photo_client::photo::PhotoDimensions,
) -> Result<Arc<Pixels>, Error> {
    let mut bytes = Vec::new();
    file.by_ref()
        .take(length + 1)
        .read_to_end(&mut bytes)
        .context(IoSnafu)?;
    ensure!(
        bytes.len() as u64 == length && hex::encode(Sha256::digest(&bytes)) == sha256,
        IntegritySnafu
    );
    let decoded = image::load_from_memory(&bytes).context(DecodeSnafu)?;
    ensure!(
        decoded.width() == dimensions.width && decoded.height() == dimensions.height,
        IntegritySnafu
    );
    // The backend preserves the full-resolution original for confirmation/export.
    // A display-sized derivative bounds retained UI memory independently of camera resolution.
    Ok(pixels(decoded.thumbnail(2560, 1440)))
}

pub(super) fn logo(path: &std::path::Path) -> Result<Arc<Pixels>, Error> {
    // Workspace::open validated these resources; only tiny display derivatives cross threads.
    let decoded = image::ImageReader::open(path)
        .context(IoSnafu)?
        .decode()
        .context(DecodeSnafu)?;
    Ok(pixels(decoded.thumbnail(160, 160)))
}

fn pixels(image: image::DynamicImage) -> Arc<Pixels> {
    let rgba = image.into_rgba8();
    Arc::new(Pixels {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}
