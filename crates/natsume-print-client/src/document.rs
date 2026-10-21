use std::{fs::File, io::Read as _, path::Path};

use snafu::ResultExt as _;
use uuid::Uuid;

use crate::{InvalidSnafu, IoSnafu, Result};

const DOCUMENT_LIMIT: u64 = 8 * 1024 * 1024;

/// Immutable bytes prevent file changes from affecting an active rendering session.
pub struct Document {
    bytes: Vec<u8>,
    label: String,
}

impl Document {
    /// Loads an immutable PDF for printing; this does not replace Server admission validation.
    ///
    /// # Errors
    /// Rejects non-regular, empty, oversized or non-PDF input and noncanonical attempt IDs.
    pub fn load(path: &Path, attempt_id: &str) -> Result<Self> {
        let label = document_label(attempt_id)?;
        let metadata = std::fs::symlink_metadata(path).context(IoSnafu {
            operation: "inspect PDF",
        })?;
        if !metadata.is_file() || metadata.len() > DOCUMENT_LIMIT {
            return InvalidSnafu { field: "PDF file" }.fail();
        }
        let file = File::open(path).context(IoSnafu {
            operation: "open PDF",
        })?;
        if !file
            .metadata()
            .context(IoSnafu {
                operation: "inspect open PDF",
            })?
            .is_file()
        {
            return InvalidSnafu { field: "PDF file" }.fail();
        }
        let mut bytes = Vec::new();
        file.take(DOCUMENT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .context(IoSnafu {
                operation: "read PDF",
            })?;
        if bytes.len() as u64 > DOCUMENT_LIMIT || !bytes.starts_with(b"%PDF-") {
            return InvalidSnafu { field: "PDF bytes" }.fail();
        }
        Ok(Self { bytes, label })
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// Builds the exact spooler label used to fence reused numeric Windows Job IDs.
///
/// # Errors
/// Rejects anything other than a canonical RFC 4122 `UUIDv7`.
fn document_label(attempt_id: &str) -> Result<String> {
    let id = Uuid::parse_str(attempt_id).map_err(|_| {
        InvalidSnafu {
            field: "attempt_id",
        }
        .build()
    })?;
    if id.get_version_num() != 7
        || id.get_variant() != uuid::Variant::RFC4122
        || id.to_string() != attempt_id
    {
        return InvalidSnafu {
            field: "attempt_id",
        }
        .fail();
    }
    Ok(format!("Natsume/{attempt_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_document_is_immutable_and_has_attempt_identity() -> Result<()> {
        let directory = tempfile::tempdir().context(IoSnafu {
            operation: "test directory",
        })?;
        let path = directory.path().join("source.pdf");
        let bytes = b"%PDF-1.7 fixture input";
        std::fs::write(&path, bytes).context(IoSnafu {
            operation: "test PDF",
        })?;
        let id = Uuid::now_v7().to_string();
        let document = Document::load(&path, &id)?;
        std::fs::write(&path, b"changed").context(IoSnafu {
            operation: "replace test PDF",
        })?;
        assert_eq!(document.bytes(), bytes);
        assert_eq!(document.label(), format!("Natsume/{id}"));
        Ok(())
    }

    #[test]
    fn rejects_bad_identity_and_non_pdf_input() -> Result<()> {
        let file = tempfile::NamedTempFile::new().context(IoSnafu {
            operation: "test file",
        })?;
        for id in ["invalid", "01967c61-1021-4a82-9236-146830456201"] {
            assert!(Document::load(file.path(), id).is_err());
        }
        std::fs::write(file.path(), b"not a PDF").context(IoSnafu {
            operation: "test bytes",
        })?;
        assert!(Document::load(file.path(), &Uuid::now_v7().to_string()).is_err());
        let huge = File::options()
            .write(true)
            .open(file.path())
            .context(IoSnafu {
                operation: "open test file",
            })?;
        huge.set_len(DOCUMENT_LIMIT + 1).context(IoSnafu {
            operation: "extend test file",
        })?;
        assert!(Document::load(file.path(), &Uuid::now_v7().to_string()).is_err());
        Ok(())
    }
}
