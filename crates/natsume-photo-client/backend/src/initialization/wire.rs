pub(super) use natsume_photo_protocol::{
    IMAGE_LIMIT, MANIFEST_LIMIT, PhotoEnrollmentIssued as PairingCredentials,
    PhotoInitialization as Manifest, PhotoLogo as Logo, hash, id,
};
use sha2::{Digest as _, Sha256};
pub(super) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
