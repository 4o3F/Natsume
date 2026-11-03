//! Version-one team results: strict JSON, RFC 8785 canonicalization and Ed25519 signatures.
//! Authorization, filesystem access and ZIP handling remain at the application boundaries.
use crate::{
    DatasetId, FormatVersion, Identity, InitializationId, MANIFEST_LIMIT, Sha256Digest,
    SigningPublicKey, SourceId, TEAM_LIMIT,
};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const RESULT_IMAGE_LIMIT: u64 = 50 * 1024 * 1024;
pub const RESULT_PIXEL_LIMIT: u64 = 80_000_000;
pub const PACKAGE_LIMIT: u64 = 8 * 1024 * 1024 * 1024;
pub const ZIP_METADATA_LIMIT: usize = 4 * 1024 * 1024;
/// Reserve the manifest and ZIP headers within the complete package budget.
pub const RESULT_IMAGES_LIMIT: u64 =
    PACKAGE_LIMIT - MANIFEST_LIMIT as u64 - ZIP_METADATA_LIMIT as u64;
/// Includes manifest.json; no directory entries are required.
pub const PACKAGE_ENTRY_LIMIT: usize = TEAM_LIMIT + 1;
pub const SIGNING_DOMAIN: &[u8] = b"Natsume/Photo/team-results/v1\0";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResultRole {}
pub type ResultId = Identity<ResultRole>;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubmissionRole {}
pub type SubmissionId = Identity<SubmissionRole>;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestRole {}
pub type ManifestId = Identity<ManifestRole>;

/// Actual encoded format, not a caller-supplied filename extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoFormat {
    Jpeg,
    Png,
}
impl PhotoFormat {
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    TeamResults,
}

/// Reuses the exact identities of the authorized initialization, including its raw JSON hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultContext {
    pub source_id: SourceId,
    pub dataset_id: DatasetId,
    pub initialization_id: InitializationId,
    pub initialization_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamResult {
    pub result_id: ResultId,
    pub submission_id: SubmissionId,
    pub account_id: String,
    pub account: String,
    pub image_sha256: Sha256Digest,
    pub image_bytes: u64,
    pub image_format: PhotoFormat,
    pub width: u32,
    pub height: u32,
}
impl TeamResult {
    /// ZIP paths are derived only from validated digest and closed format values.
    #[must_use]
    pub fn image_entry(&self) -> String {
        format!(
            "images/{}.{}",
            self.image_sha256,
            self.image_format.extension()
        )
    }
}

/// Array order is `account_id` UTF-8 byte order; JCS itself does not reorder arrays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultsManifest {
    pub format_version: FormatVersion,
    pub kind: ManifestKind,
    pub manifest_id: ManifestId,
    pub context: ResultContext,
    pub signing_public_key: SigningPublicKey,
    pub results: Vec<TeamResult>,
}

#[derive(Debug, Snafu)]
pub enum ResultError {
    #[snafu(display("result manifest exceeds its byte limit"))]
    ManifestLimit,
    #[snafu(display("result manifest JSON is invalid"))]
    Json { source: serde_json::Error },
    #[snafu(display("result manifest must contain between one and 2000 results"))]
    Count,
    #[snafu(display("result identities are empty, duplicated or out of canonical order"))]
    Identity,
    #[snafu(display("result image metadata or resource budget is invalid"))]
    Image,
    #[snafu(display("shared image references disagree"))]
    SharedImage,
    #[snafu(display("result manifest does not match the trusted initialization or key"))]
    Authorization,
    #[snafu(display("result signature must be 128 lowercase hexadecimal digits"))]
    SignatureEncoding,
    #[snafu(display("result cryptographic value encoding failed"))]
    Hex { source: hex::FromHexError },
    #[snafu(display("result signature verification failed"))]
    Signature {
        source: ed25519_dalek::SignatureError,
    },
}
type Result<T> = std::result::Result<T, ResultError>;

/// Canonically encoded Ed25519 signature, not a source authorization credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ResultSignature(String);
impl ResultSignature {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ResultSignature {
    type Error = ResultError;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            value.len() == 128
                && value
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            SignatureEncodingSnafu
        );
        Ok(Self(value))
    }
}
impl From<ResultSignature> for String {
    fn from(value: ResultSignature) -> Self {
        value.0
    }
}

/// manifest.json contains this envelope; the signature covers domain || JCS(manifest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedResults {
    pub manifest: ResultsManifest,
    pub signature: ResultSignature,
}

impl ResultsManifest {
    /// Checks semantic bounds and deterministic order before signing or accepting bytes.
    /// # Errors
    /// Rejects empty/duplicate identities, noncanonical order and invalid image budgets.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.results.is_empty() && self.results.len() <= TEAM_LIMIT,
            CountSnafu
        );
        let mut previous: Option<&str> = None;
        let mut accounts = BTreeSet::new();
        let mut results = BTreeSet::new();
        let mut submissions = BTreeSet::new();
        let mut images = BTreeMap::new();
        let mut image_bytes = 0_u64;
        let mut text_bytes = 0;
        for entry in &self.results {
            ensure!(
                !entry.account_id.trim().is_empty()
                    && !entry.account.trim().is_empty()
                    && previous.is_none_or(|last| last.as_bytes() < entry.account_id.as_bytes())
                    && accounts.insert(&entry.account)
                    && results.insert(&entry.result_id)
                    && submissions.insert(&entry.submission_id),
                IdentitySnafu
            );
            previous = Some(&entry.account_id);
            // Locally constructed values must also be bounded before canonical allocation.
            text_bytes += entry.account_id.len() + entry.account.len();
            ensure!(text_bytes <= MANIFEST_LIMIT, ManifestLimitSnafu);
            ensure!(
                entry.image_bytes > 0
                    && entry.image_bytes <= RESULT_IMAGE_LIMIT
                    && entry.width > 0
                    && entry.height > 0
                    && u64::from(entry.width) * u64::from(entry.height) <= RESULT_PIXEL_LIMIT,
                ImageSnafu
            );
            let metadata = (
                entry.image_bytes,
                entry.image_format,
                entry.width,
                entry.height,
            );
            if let Some(existing) = images.insert(&entry.image_sha256, metadata) {
                ensure!(existing == metadata, SharedImageSnafu);
            } else {
                image_bytes += entry.image_bytes;
                ensure!(image_bytes <= RESULT_IMAGES_LIMIT, ImageSnafu);
            }
        }
        Ok(())
    }

    /// Returns the exact domain-separated RFC 8785 bytes for independent implementations.
    /// # Errors
    /// Reports invalid fields, canonicalization failures or an oversized payload.
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let json = serde_json_canonicalizer::to_vec(self).context(JsonSnafu)?;
        ensure!(json.len() <= MANIFEST_LIMIT, ManifestLimitSnafu);
        let mut bytes = Vec::with_capacity(SIGNING_DOMAIN.len() + json.len());
        bytes.extend_from_slice(SIGNING_DOMAIN);
        bytes.extend_from_slice(&json);
        Ok(bytes)
    }

    /// Signs with the already-authorized key; never creates or stores credentials.
    /// # Errors
    /// Rejects the wrong key, invalid payload or an oversized signed envelope.
    pub fn sign(&self, key: &SigningKey) -> Result<SignedResults> {
        ensure!(
            hex::encode(key.verifying_key().to_bytes()) == self.signing_public_key.as_str(),
            AuthorizationSnafu
        );
        let signature = key.sign(&self.signing_bytes()?);
        let signed = SignedResults {
            manifest: self.clone(),
            signature: ResultSignature(hex::encode(signature.to_bytes())),
        };
        signed.to_json()?;
        Ok(signed)
    }
}

impl SignedResults {
    /// Encodes a bounded envelope; encoding alone does not authorize or verify a signature.
    /// # Errors
    /// Reports invalid payload, encoding errors or a manifest larger than 4 MiB.
    pub fn to_json(&self) -> Result<Vec<u8>> {
        self.manifest.validate()?;
        let bytes = serde_json_canonicalizer::to_vec(self).context(JsonSnafu)?;
        ensure!(bytes.len() <= MANIFEST_LIMIT, ManifestLimitSnafu);
        Ok(bytes)
    }

    /// Verifies against externally trusted source/context/key, never the envelope alone.
    /// # Errors
    /// Reports authorization mismatch, invalid fields or a bad Ed25519 signature.
    pub fn verify(&self, expected: &ResultContext, key: &SigningPublicKey) -> Result<()> {
        ensure!(
            &self.manifest.context == expected && &self.manifest.signing_public_key == key,
            AuthorizationSnafu
        );
        let bytes = self.manifest.signing_bytes()?;
        let mut public = [0; 32];
        hex::decode_to_slice(key.as_str(), &mut public).context(HexSnafu)?;
        let public = VerifyingKey::from_bytes(&public).context(SignatureSnafu)?;
        let mut signature = [0; 64];
        hex::decode_to_slice(self.signature.as_str(), &mut signature).context(HexSnafu)?;
        public
            .verify_strict(&bytes, &Signature::from_bytes(&signature))
            .context(SignatureSnafu)
    }

    /// Bounds the raw input before typed decoding; duplicate and unknown fields are rejected.
    /// # Errors
    /// Reports oversized/invalid JSON, authorization mismatch or invalid signatures.
    pub fn parse_and_verify(
        bytes: &[u8],
        expected: &ResultContext,
        key: &SigningPublicKey,
    ) -> Result<Self> {
        ensure!(bytes.len() <= MANIFEST_LIMIT, ManifestLimitSnafu);
        let signed: Self = serde_json::from_slice(bytes).context(JsonSnafu)?;
        signed.verify(expected, key)?;
        Ok(signed)
    }
}
