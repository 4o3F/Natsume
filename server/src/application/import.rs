use std::{
    collections::HashSet,
    error::Error,
    fmt::{self, Display, Formatter},
    path::Path,
    str,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    audit::CorrelationId,
    db::{self, Database},
    vault,
};

pub(crate) const IMPORT_CANDIDATE_TTL_SECONDS: i64 = 1_800;
pub(crate) const MAX_IMPORT_ROWS: usize = 10_000;

const CSV_HEADER: &str = "seat,account,password";
const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
const SEAT_CODE_LENGTH_LIMIT: usize = 64;
const ACCOUNT_USERNAME_LENGTH_LIMIT: usize = 64;
const PASSWORD_LENGTH_LIMIT: usize = 512;
const PREVIEW_TOKEN_LENGTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CsvImportErrorCategory {
    InvalidUtf8,
    InvalidHeader,
    WrongColumnCount,
    EmptyField,
    FieldTooLong,
    ControlCharacter,
    DuplicateSeatCode,
    DuplicateAccountUsername,
    TooManyRows,
    ZeroDataRows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CsvImportError {
    line: usize,
    category: CsvImportErrorCategory,
}

impl CsvImportError {
    const fn new(line: usize, category: CsvImportErrorCategory) -> Self {
        Self { line, category }
    }

    #[must_use]
    pub(crate) const fn line(&self) -> usize {
        self.line
    }

    #[must_use]
    pub(crate) const fn category(&self) -> CsvImportErrorCategory {
        self.category
    }
}

impl Display for CsvImportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("the CSV import is invalid")
    }
}

impl Error for CsvImportError {}

#[derive(Serialize, Zeroize, ZeroizeOnDrop)]
pub(crate) struct ImportRow {
    seat_code: String,
    domjudge_username: String,
    password: String,
}

impl ImportRow {
    #[must_use]
    pub(crate) fn seat_code(&self) -> &str {
        &self.seat_code
    }

    #[must_use]
    pub(crate) fn domjudge_username(&self) -> &str {
        &self.domjudge_username
    }

    #[must_use]
    pub(crate) fn password(&self) -> &str {
        &self.password
    }
}

pub(crate) struct ParsedImport {
    rows: Vec<ImportRow>,
    staging_plaintext: Zeroizing<Vec<u8>>,
}

impl ParsedImport {
    #[must_use]
    pub(crate) fn rows(&self) -> &[ImportRow] {
        &self.rows
    }

    #[must_use]
    pub(crate) fn staging_plaintext(&self) -> &[u8] {
        &self.staging_plaintext
    }

    fn candidate_rows(&self) -> Vec<CandidateRowFacts> {
        self.rows
            .iter()
            .map(|row| CandidateRowFacts {
                seat_code: row.seat_code.clone(),
                domjudge_username: row.domjudge_username.clone(),
            })
            .collect()
    }
}

/// Parses the frozen import CSV grammar and builds its canonical staging JSON.
///
/// # Errors
///
/// Returns a typed, content-free [`CsvImportError`] for the first invalid line.
///
/// # Panics
///
/// Panics only if serializing the fixed string-only staging shape violates its
/// infallible serialization invariant.
pub(crate) fn parse_csv(raw: &[u8]) -> Result<ParsedImport, CsvImportError> {
    let bytes = raw.strip_prefix(UTF8_BOM).unwrap_or(raw);
    let text = str::from_utf8(bytes).map_err(|error| {
        let line = bytes[..error.valid_up_to()]
            .split(|byte| *byte == b'\n')
            .count();
        CsvImportError::new(line, CsvImportErrorCategory::InvalidUtf8)
    })?;

    let mut lines = text.split_inclusive('\n');
    let Some(header) = lines.next() else {
        return Err(CsvImportError::new(1, CsvImportErrorCategory::ZeroDataRows));
    };
    if csv_line(header) != CSV_HEADER {
        return Err(CsvImportError::new(
            1,
            CsvImportErrorCategory::InvalidHeader,
        ));
    }

    let mut rows = Vec::new();
    let mut seat_codes = HashSet::new();
    let mut account_usernames = HashSet::new();
    for (index, encoded_line) in lines.enumerate() {
        let line_number = index + 2;
        if rows.len() == MAX_IMPORT_ROWS {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::TooManyRows,
            ));
        }
        let line = csv_line(encoded_line);
        let fields = line.split(',').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::WrongColumnCount,
            ));
        }
        if fields.iter().any(|field| field.is_empty()) {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::EmptyField,
            ));
        }
        if fields[0].len() > SEAT_CODE_LENGTH_LIMIT
            || fields[1].len() > ACCOUNT_USERNAME_LENGTH_LIMIT
            || fields[2].len() > PASSWORD_LENGTH_LIMIT
        {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::FieldTooLong,
            ));
        }
        if fields
            .iter()
            .any(|field| field.chars().any(char::is_control))
        {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::ControlCharacter,
            ));
        }
        if !seat_codes.insert(fields[0].to_owned()) {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::DuplicateSeatCode,
            ));
        }
        if !account_usernames.insert(fields[1].to_owned()) {
            return Err(CsvImportError::new(
                line_number,
                CsvImportErrorCategory::DuplicateAccountUsername,
            ));
        }
        rows.push(ImportRow {
            seat_code: fields[0].to_owned(),
            domjudge_username: fields[1].to_owned(),
            password: fields[2].to_owned(),
        });
    }

    if rows.is_empty() {
        return Err(CsvImportError::new(1, CsvImportErrorCategory::ZeroDataRows));
    }
    let capacity = bytes
        .len()
        .saturating_mul(2)
        .saturating_add(rows.len().saturating_mul(64))
        .saturating_add(2);
    let mut staging_plaintext = Zeroizing::new(Vec::with_capacity(capacity));
    serde_json::to_writer(&mut *staging_plaintext, &rows)
        .unwrap_or_else(|_| panic!("import staging serialization invariant failed"));
    Ok(ParsedImport {
        rows,
        staging_plaintext,
    })
}

fn csv_line(encoded_line: &str) -> &str {
    encoded_line
        .strip_suffix('\n')
        .map_or(encoded_line, |line| line.strip_suffix('\r').unwrap_or(line))
}

pub(crate) struct CandidateRowFacts {
    pub(crate) seat_code: String,
    pub(crate) domjudge_username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportMappingChange {
    seat_code: String,
    current_domjudge_username: Option<String>,
    candidate_domjudge_username: String,
}

impl ImportMappingChange {
    pub(crate) fn new(
        seat_code: String,
        current_domjudge_username: Option<String>,
        candidate_domjudge_username: String,
    ) -> Self {
        Self {
            seat_code,
            current_domjudge_username,
            candidate_domjudge_username,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ImportBindingImpact {
    seat_code: String,
    device_id: String,
}

impl ImportBindingImpact {
    pub(crate) fn new(seat_code: String, device_id: String) -> Self {
        Self {
            seat_code,
            device_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RedactedImportPreview {
    seats_added: Vec<String>,
    seats_removed: Vec<String>,
    mappings_changed: Vec<ImportMappingChange>,
    unchanged_count: usize,
    affected_account_count: usize,
    binding_impacts: Vec<ImportBindingImpact>,
}

impl RedactedImportPreview {
    pub(crate) fn new(
        seats_added: Vec<String>,
        seats_removed: Vec<String>,
        mappings_changed: Vec<ImportMappingChange>,
        unchanged_count: usize,
        affected_account_count: usize,
        binding_impacts: Vec<ImportBindingImpact>,
    ) -> Self {
        Self {
            seats_added,
            seats_removed,
            mappings_changed,
            unchanged_count,
            affected_account_count,
            binding_impacts,
        }
    }

    #[must_use]
    pub(crate) fn seats_added(&self) -> &[String] {
        &self.seats_added
    }

    #[must_use]
    pub(crate) fn seats_removed(&self) -> &[String] {
        &self.seats_removed
    }

    #[must_use]
    pub(crate) fn mappings_changed(&self) -> &[ImportMappingChange] {
        &self.mappings_changed
    }

    #[must_use]
    pub(crate) const fn unchanged_count(&self) -> usize {
        self.unchanged_count
    }

    #[must_use]
    pub(crate) const fn affected_account_count(&self) -> usize {
        self.affected_account_count
    }

    #[must_use]
    pub(crate) fn binding_impacts(&self) -> &[ImportBindingImpact] {
        &self.binding_impacts
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct PreviewToken {
    bytes: [u8; PREVIEW_TOKEN_LENGTH],
}

impl PreviewToken {
    fn generate() -> Result<Self, ImportError> {
        let mut token = Self {
            bytes: [0_u8; PREVIEW_TOKEN_LENGTH],
        };
        getrandom::fill(&mut token.bytes).map_err(|_| ImportError::EntropyUnavailable)?;
        Ok(token)
    }

    fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.bytes.as_slice()).into()
    }

    #[must_use]
    pub(crate) const fn as_bytes(&self) -> &[u8; PREVIEW_TOKEN_LENGTH] {
        &self.bytes
    }
}

pub(crate) struct CreatedImportCandidate {
    candidate_id: Uuid,
    preview_token: PreviewToken,
    expires_at: String,
    baseline_configuration_revision: i64,
    baseline_binding_revision: i64,
    diff: RedactedImportPreview,
}

impl CreatedImportCandidate {
    #[must_use]
    pub(crate) const fn candidate_id(&self) -> Uuid {
        self.candidate_id
    }

    #[must_use]
    pub(crate) const fn preview_token(&self) -> &PreviewToken {
        &self.preview_token
    }

    #[must_use]
    pub(crate) fn expires_at(&self) -> &str {
        &self.expires_at
    }

    #[must_use]
    pub(crate) const fn baseline_configuration_revision(&self) -> i64 {
        self.baseline_configuration_revision
    }

    #[must_use]
    pub(crate) const fn baseline_binding_revision(&self) -> i64 {
        self.baseline_binding_revision
    }

    #[must_use]
    pub(crate) const fn diff(&self) -> &RedactedImportPreview {
        &self.diff
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportError {
    InvalidCsv(CsvImportError),
    CandidateInvalid,
    CandidatePending,
    EntropyUnavailable,
    VaultFailure,
    PersistenceFailure,
}

impl Display for ImportError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCsv(_) | Self::CandidateInvalid => "the import candidate is invalid",
            Self::CandidatePending => "an import candidate is already pending",
            Self::EntropyUnavailable => "import candidate entropy is unavailable",
            Self::VaultFailure => "the import payload could not be staged",
            Self::PersistenceFailure => "the import candidate could not be persisted",
        })
    }
}

impl Error for ImportError {}

/// Strictly parses, encrypts, diffs, and atomically persists one import candidate.
///
/// # Errors
///
/// Returns a typed [`ImportError`] for invalid input, a live singleton candidate,
/// entropy failure, vault failure, or persistence failure.
pub(crate) async fn create_import_candidate(
    database: &Database,
    master_key_path: &Path,
    raw_csv: &[u8],
    correlation_id: CorrelationId,
) -> Result<CreatedImportCandidate, ImportError> {
    let parsed = parse_csv(raw_csv).map_err(ImportError::InvalidCsv)?;
    let preview_token = PreviewToken::generate()?;
    let preview_token_hash = preview_token.sha256();
    let candidate_rows = parsed.candidate_rows();
    let (nonce, ciphertext) = vault::seal(master_key_path, parsed.staging_plaintext())
        .map_err(|_| ImportError::VaultFailure)?;
    drop(parsed);

    let created = db::import::create_import_candidate(
        database,
        candidate_rows,
        preview_token_hash,
        nonce,
        ciphertext,
        correlation_id,
    )
    .await?;

    Ok(CreatedImportCandidate {
        candidate_id: created.candidate_id,
        preview_token,
        expires_at: created.expires_at,
        baseline_configuration_revision: created.baseline_configuration_revision,
        baseline_binding_revision: created.baseline_binding_revision,
        diff: created.diff,
    })
}

#[cfg(test)]
mod parser_tests {
    use std::fmt::Write as _;

    use super::{CsvImportErrorCategory, MAX_IMPORT_ROWS, parse_csv};

    struct RejectionCase {
        input: Vec<u8>,
        category: CsvImportErrorCategory,
        line: usize,
        canary: &'static str,
    }

    #[test]
    fn strict_parser_rejection_matrix_is_typed_and_redacted() {
        let mut invalid_utf8 = b"seat,account,password\nS-01,team-1,invalid-utf8-canary-".to_vec();
        invalid_utf8.push(0xff);
        let long_seat = format!(
            "seat,account,password\n{},team-1,field-length-canary",
            "S".repeat(65)
        )
        .into_bytes();
        let mut too_many_rows = String::from("seat,account,password");
        for index in 0..=MAX_IMPORT_ROWS {
            assert!(write!(too_many_rows, "\nS-{index},team-{index},row-cap-canary").is_ok());
        }
        let cases = [
            RejectionCase {
                input: invalid_utf8,
                category: CsvImportErrorCategory::InvalidUtf8,
                line: 2,
                canary: "invalid-utf8-canary",
            },
            RejectionCase {
                input: b"seat,account,wrong-header-canary".to_vec(),
                category: CsvImportErrorCategory::InvalidHeader,
                line: 1,
                canary: "wrong-header-canary",
            },
            RejectionCase {
                input: b"S-01,team-1,missing-header-canary".to_vec(),
                category: CsvImportErrorCategory::InvalidHeader,
                line: 1,
                canary: "missing-header-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,column-count-canary,extra".to_vec(),
                category: CsvImportErrorCategory::WrongColumnCount,
                line: 2,
                canary: "column-count-canary",
            },
            RejectionCase {
                input: b"seat,account,password\n,team-1,empty-field-canary".to_vec(),
                category: CsvImportErrorCategory::EmptyField,
                line: 2,
                canary: "empty-field-canary",
            },
            RejectionCase {
                input: long_seat,
                category: CsvImportErrorCategory::FieldTooLong,
                line: 2,
                canary: "field-length-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,control-canary\x07".to_vec(),
                category: CsvImportErrorCategory::ControlCharacter,
                line: 2,
                canary: "control-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,first-password\nS-01,team-2,duplicate-seat-canary".to_vec(),
                category: CsvImportErrorCategory::DuplicateSeatCode,
                line: 3,
                canary: "duplicate-seat-canary",
            },
            RejectionCase {
                input: b"seat,account,password\nS-01,team-1,first-password\nS-02,team-1,duplicate-account-canary".to_vec(),
                category: CsvImportErrorCategory::DuplicateAccountUsername,
                line: 3,
                canary: "duplicate-account-canary",
            },
            RejectionCase {
                input: too_many_rows.into_bytes(),
                category: CsvImportErrorCategory::TooManyRows,
                line: MAX_IMPORT_ROWS + 2,
                canary: "row-cap-canary",
            },
            RejectionCase {
                input: Vec::new(),
                category: CsvImportErrorCategory::ZeroDataRows,
                line: 1,
                canary: "",
            },
            RejectionCase {
                input: b"seat,account,password\r\n".to_vec(),
                category: CsvImportErrorCategory::ZeroDataRows,
                line: 1,
                canary: "",
            },
        ];

        for case in cases {
            let Err(error) = parse_csv(&case.input) else {
                panic!("invalid CSV was accepted")
            };
            assert_eq!(error.category(), case.category);
            assert_eq!(error.line(), case.line);
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert_eq!(display, "the CSV import is invalid");
            if !case.canary.is_empty() {
                assert!(!display.contains(case.canary));
                assert!(!debug.contains(case.canary));
            }
        }
    }

    #[test]
    fn parser_preserves_order_and_emits_golden_canonical_json() {
        let Ok(parsed) = parse_csv(
            b"seat,account,password\nB-02,team-beta,beta-password\nA-01,team-alpha,alpha-password",
        ) else {
            panic!("valid CSV was rejected")
        };
        assert_eq!(parsed.rows().len(), 2);
        assert_eq!(parsed.rows()[0].seat_code(), "B-02");
        assert_eq!(parsed.rows()[0].domjudge_username(), "team-beta");
        assert_eq!(parsed.rows()[0].password(), "beta-password");
        assert_eq!(parsed.rows()[1].seat_code(), "A-01");
        assert_eq!(
            parsed.staging_plaintext(),
            br#"[{"seat_code":"B-02","domjudge_username":"team-beta","password":"beta-password"},{"seat_code":"A-01","domjudge_username":"team-alpha","password":"alpha-password"}]"#
        );
    }

    #[test]
    fn parser_accepts_one_leading_bom_and_crlf_lines() {
        let Ok(with_bom) =
            parse_csv(b"\xef\xbb\xbfseat,account,password\nA-01,team-alpha,bom-password\n")
        else {
            panic!("BOM CSV was rejected")
        };
        assert_eq!(with_bom.rows()[0].password(), "bom-password");

        let Ok(with_crlf) =
            parse_csv(b"seat,account,password\r\nA-01,team-alpha,crlf-password\r\n")
        else {
            panic!("CRLF CSV was rejected")
        };
        assert_eq!(with_crlf.rows()[0].seat_code(), "A-01");
        assert_eq!(with_crlf.rows()[0].password(), "crlf-password");
        assert!(!with_crlf.staging_plaintext().contains(&b'\r'));
    }
}

#[cfg(test)]
mod candidate_tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use diesel::{
        Connection, QueryableByName, RunQueryDsl,
        connection::SimpleConnection,
        sql_types::{BigInt, Binary, Nullable, Text},
        sqlite::SqliteConnection,
    };
    use sha2::{Digest, Sha256};
    use snafu::Snafu;
    use uuid::Uuid;

    use crate::{
        audit::CorrelationId,
        db::{Database, DatabaseConfig},
        vault::{ensure_master_key, open},
    };

    use super::{CsvImportErrorCategory, ImportError, create_import_candidate};

    const DEVICE_C: &str = "01900000-0000-7000-8000-000000000201";
    const DEVICE_D: &str = "01900000-0000-7000-8000-000000000202";
    const DEVICE_A: &str = "01900000-0000-7000-8000-000000000203";

    #[tokio::test]
    async fn invalid_upload_stops_before_vault_or_database_access() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let mut observer = fixture.observer()?;
        let before = persistence_snapshot(&fixture.database).await?;
        let data_version_before = data_version(&mut observer)?;
        let missing_key_path = fixture.directory_path.join("missing-master.key");

        let Err(error) = create_import_candidate(
            &fixture.database,
            &missing_key_path,
            b"seat,account,password\nA-01,team-a,parse-password-canary,extra",
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::InvalidUploadWasAccepted);
        };
        let ImportError::InvalidCsv(parse_error) = error else {
            return Err(TestFailure::InvalidUploadReachedVault);
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        if parse_error.category() != CsvImportErrorCategory::WrongColumnCount
            || parse_error.line() != 2
            || display.contains("parse-password-canary")
            || debug.contains("parse-password-canary")
        {
            return Err(TestFailure::PendingErrorChanged);
        }

        let after = persistence_snapshot(&fixture.database).await?;
        let data_version_after = data_version(&mut observer)?;
        if before != after || data_version_before != data_version_after || missing_key_path.exists()
        {
            return Err(TestFailure::InvalidUploadWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn candidate_creation_persists_encrypted_payload_and_golden_redacted_diff()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_golden_current_facts(&fixture.database).await?;
        let correlation_id = correlation_id();
        let csv = b"seat,account,password\n\
                    F-06,new-f,password-f\n\
                    E-05,new-e,password-e\n\
                    A-01,same-a,password-a\n\
                    B-02,new-b,password-b\n\
                    G-07,new-g,password-g";

        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            csv,
            correlation_id,
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let evidence = candidate_evidence(&fixture.database).await?;
        let expected_preview = r#"{"seats_added":["F-06","G-07"],"seats_removed":["C-03","D-04"],"mappings_changed":[{"seat_code":"B-02","current_domjudge_username":"old-b","candidate_domjudge_username":"new-b"},{"seat_code":"E-05","current_domjudge_username":null,"candidate_domjudge_username":"new-e"}],"unchanged_count":1,"affected_account_count":5,"binding_impacts":[{"seat_code":"C-03","device_id":"01900000-0000-7000-8000-000000000201"},{"seat_code":"D-04","device_id":"01900000-0000-7000-8000-000000000202"}]}"#;
        let expected_staging = br#"[{"seat_code":"F-06","domjudge_username":"new-f","password":"password-f"},{"seat_code":"E-05","domjudge_username":"new-e","password":"password-e"},{"seat_code":"A-01","domjudge_username":"same-a","password":"password-a"},{"seat_code":"B-02","domjudge_username":"new-b","password":"password-b"},{"seat_code":"G-07","domjudge_username":"new-g","password":"password-g"}]"#;

        if created.candidate_id().to_string() != evidence.candidate_id
            || created.expires_at() != evidence.expires_at
            || created.baseline_configuration_revision() != 23
            || created.baseline_binding_revision() != 31
            || evidence.baseline_configuration_revision != 23
            || evidence.baseline_binding_revision != 31
            || evidence.ttl_valid != 1
            || evidence.record_type != "import_payload"
            || evidence.subject_id != evidence.candidate_id
            || evidence.nonce.len() != 24
            || evidence.redacted_preview_json != expected_preview
            || serde_json::to_string(created.diff()).map_err(|_| TestFailure::EvidenceFailed)?
                != expected_preview
        {
            return Err(TestFailure::CandidateEvidenceChanged);
        }
        assert_canonical_uuid_v7(&evidence.candidate_id)?;
        assert_canonical_uuid_v7(&evidence.payload_vault_record_id)?;

        let expected_hash = Sha256::digest(created.preview_token().as_bytes());
        if evidence.preview_token_hash.as_slice() != expected_hash.as_slice() {
            return Err(TestFailure::PreviewTokenHashChanged);
        }
        assert_database_files_exclude(&fixture.database_path, created.preview_token().as_bytes())?;

        let opened = open(
            &fixture.master_key_path,
            &evidence.nonce,
            &evidence.ciphertext,
        )
        .map_err(|_| TestFailure::PayloadOpenFailed)?;
        if opened.as_slice() != expected_staging
            || evidence.ciphertext.as_slice() == expected_staging
        {
            return Err(TestFailure::PayloadEvidenceChanged);
        }
        assert_database_files_exclude(&fixture.database_path, expected_staging)?;

        let audit = audit_for_resource(&fixture.database, &evidence.candidate_id).await?;
        if audit.actor != "operator:self"
            || audit.action_kind != "create_import_candidate"
            || audit.resource_type != "import_candidate"
            || audit.resource_id.as_deref() != Some(evidence.candidate_id.as_str())
            || audit.result != "succeeded"
            || audit.reason_code.as_deref() != Some("operator_requested")
            || audit.correlation_id != correlation_id.as_text()
            || audit.redacted_detail_json
                != r#"{"seats_added_count":2,"seats_removed_count":2,"mappings_changed_count":2,"binding_impact_count":2}"#
        {
            return Err(TestFailure::AuditEvidenceChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn second_upload_while_pending_commits_zero_writes() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let first = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,first-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let before = persistence_snapshot(&fixture.database).await?;
        let mut observer = fixture.observer()?;
        let data_version_before = data_version(&mut observer)?;

        let Err(error) = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,pending-password-canary",
            correlation_id(),
        )
        .await
        else {
            return Err(TestFailure::PendingCandidateWasReplaced);
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        if error != ImportError::CandidatePending
            || display.contains("pending-password-canary")
            || debug.contains("pending-password-canary")
        {
            return Err(TestFailure::PendingErrorChanged);
        }

        let after = persistence_snapshot(&fixture.database).await?;
        let data_version_after = data_version(&mut observer)?;
        if before != after
            || data_version_before != data_version_after
            || after.candidate_id.as_deref() != Some(first.candidate_id().to_string().as_str())
        {
            return Err(TestFailure::PendingUploadWroteData);
        }
        Ok(())
    }

    #[tokio::test]
    async fn expired_pending_candidate_is_replaced_atomically_in_one_upload()
    -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        let first = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nA-01,team-a,old-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let old = candidate_evidence(&fixture.database).await?;
        expire_current_candidate(&fixture.database).await?;

        let expiry_correlation = correlation_id();
        let second = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,new-password",
            expiry_correlation,
        )
        .await
        .map_err(|_| TestFailure::ExpiredReplacementFailed)?;
        let current = candidate_evidence(&fixture.database).await?;
        if first.candidate_id() == second.candidate_id()
            || old.candidate_id == current.candidate_id
            || current.candidate_id != second.candidate_id().to_string()
            || vault_record_count(&fixture.database, &old.payload_vault_record_id).await? != 0
            || vault_record_count(&fixture.database, &current.payload_vault_record_id).await? != 1
        {
            return Err(TestFailure::ExpiredCandidateWasNotReplaced);
        }

        let expiry_audit = expiry_audit(&fixture.database, &old.candidate_id).await?;
        if expiry_audit.count != 1
            || expiry_audit.actor != "system:expiry"
            || expiry_audit.action_kind != "expire_import_candidate"
            || expiry_audit.resource_type != "import_candidate"
            || expiry_audit.resource_id.as_deref() != Some(old.candidate_id.as_str())
            || expiry_audit.result != "succeeded"
            || expiry_audit.reason_code.as_deref() != Some("absolute_expiry_observed")
            || expiry_audit.correlation_id != expiry_correlation.as_text()
            || expiry_audit.redacted_detail_json != "{}"
            || import_audit_count(&fixture.database).await? != 3
        {
            return Err(TestFailure::ExpiryAuditChanged);
        }
        Ok(())
    }

    #[tokio::test]
    async fn identical_candidate_has_an_empty_non_secret_diff() -> Result<(), TestFailure> {
        let fixture = ImportFixture::new().await?;
        seed_identical_current_facts(&fixture.database).await?;
        let created = create_import_candidate(
            &fixture.database,
            &fixture.master_key_path,
            b"seat,account,password\nB-02,team-b,new-b-password\nA-01,team-a,new-a-password",
            correlation_id(),
        )
        .await
        .map_err(|_| TestFailure::CandidateCreationFailed)?;
        let expected = r#"{"seats_added":[],"seats_removed":[],"mappings_changed":[],"unchanged_count":2,"affected_account_count":2,"binding_impacts":[]}"#;
        let serialized =
            serde_json::to_string(created.diff()).map_err(|_| TestFailure::EvidenceFailed)?;
        let persisted = candidate_evidence(&fixture.database).await?;
        if serialized != expected
            || persisted.redacted_preview_json != expected
            || !created.diff().seats_added().is_empty()
            || !created.diff().seats_removed().is_empty()
            || !created.diff().mappings_changed().is_empty()
            || created.diff().unchanged_count() != 2
            || created.diff().affected_account_count() != 2
            || !created.diff().binding_impacts().is_empty()
        {
            return Err(TestFailure::EmptyDiffChanged);
        }
        Ok(())
    }

    async fn seed_golden_current_facts(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                connection.batch_execute(&format!(
                    "UPDATE revision_counters \
                     SET configuration_revision = 23, binding_revision = 31 WHERE singleton = 1; \
                     INSERT INTO server_vault_records \
                     (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                     ('vault-a', 'account_credential', 'account-a', x'01', x'11'), \
                     ('vault-b', 'account_credential', 'account-b', x'02', x'12'), \
                     ('vault-c', 'account_credential', 'account-c', x'03', x'13'), \
                     ('vault-d', 'account_credential', 'account-d', x'04', x'14'), \
                     ('vault-e', 'account_credential', 'account-e', x'05', x'15'); \
                     INSERT INTO seats (seat_id, seat_code) VALUES \
                     ('seat-d', 'D-04'), ('seat-b', 'B-02'), ('seat-e', 'E-05'), \
                     ('seat-a', 'A-01'), ('seat-c', 'C-03'); \
                     INSERT INTO accounts \
                     (account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES \
                     ('account-a', 'same-a', 'vault-a', 2), \
                     ('account-b', 'old-b', 'vault-b', 3), \
                     ('account-c', 'old-c', 'vault-c', 4), \
                     ('account-d', 'old-d', 'vault-d', 5), \
                     ('account-e', 'old-e', 'vault-e', 6); \
                     INSERT INTO account_mappings (seat_id, account_id) VALUES \
                     ('seat-d', 'account-d'), ('seat-b', 'account-b'), \
                     ('seat-a', 'account-a'), ('seat-c', 'account-c'); \
                     INSERT INTO devices \
                     (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                     ('{DEVICE_D}', 'machine-d', 'strong', 'enrolled'), \
                     ('{DEVICE_C}', 'machine-c', 'medium', 'enrolled'); \
                     INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                     ('seat-d', '{DEVICE_D}', 29), ('seat-c', '{DEVICE_C}', 17);"
                ))
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn seed_identical_current_facts(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                connection.batch_execute(&format!(
                    "UPDATE revision_counters \
                     SET configuration_revision = 7, binding_revision = 9 WHERE singleton = 1; \
                     INSERT INTO server_vault_records \
                     (vault_record_id, record_type, subject_id, nonce, ciphertext) VALUES \
                     ('same-vault-a', 'account_credential', 'same-account-a', x'01', x'11'), \
                     ('same-vault-b', 'account_credential', 'same-account-b', x'02', x'12'); \
                     INSERT INTO seats (seat_id, seat_code) VALUES \
                     ('same-seat-b', 'B-02'), ('same-seat-a', 'A-01'); \
                     INSERT INTO accounts \
                     (account_id, domjudge_username, credential_vault_record_id, credential_revision) VALUES \
                     ('same-account-b', 'team-b', 'same-vault-b', 4), \
                     ('same-account-a', 'team-a', 'same-vault-a', 3); \
                     INSERT INTO account_mappings (seat_id, account_id) VALUES \
                     ('same-seat-b', 'same-account-b'), ('same-seat-a', 'same-account-a'); \
                     INSERT INTO devices \
                     (device_pk, machine_hardware_id, hardware_identity_quality, state) VALUES \
                     ('{DEVICE_A}', 'machine-a', 'strong', 'enrolled'); \
                     INSERT INTO device_bindings (seat_id, device_pk, binding_revision) VALUES \
                     ('same-seat-a', '{DEVICE_A}', 9);"
                ))
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn candidate_evidence(database: &Database) -> Result<CandidateEvidence, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT p.candidate_id, p.expires_at, \
                     p.baseline_configuration_revision, p.baseline_binding_revision, \
                     p.preview_token_hash, p.payload_vault_record_id, \
                     p.redacted_preview_json, v.record_type, v.subject_id, v.nonce, v.ciphertext, \
                     CAST(p.expires_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+1799 seconds') \
                       AND p.expires_at <= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+1800 seconds') \
                       AS INTEGER) AS ttl_valid \
                     FROM pending_import_candidate p \
                     JOIN server_vault_records v \
                       ON v.vault_record_id = p.payload_vault_record_id \
                     WHERE p.singleton = 1",
                )
                .get_result::<CandidateEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn audit_for_resource(
        database: &Database,
        resource_id: &str,
    ) -> Result<AuditEvidence, TestFailure> {
        let resource_id = resource_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT actor, action_kind, resource_type, resource_id, result, reason_code, \
                     correlation_id, redacted_detail_json \
                     FROM audit_events WHERE resource_id = ? \
                     AND action_kind = 'create_import_candidate'",
                )
                .bind::<Text, _>(resource_id)
                .get_result::<AuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn expiry_audit(
        database: &Database,
        resource_id: &str,
    ) -> Result<ExpiryAuditEvidence, TestFailure> {
        let resource_id = resource_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS count, actor, action_kind, resource_type, resource_id, \
                     result, reason_code, correlation_id, redacted_detail_json \
                     FROM audit_events WHERE resource_id = ? \
                     AND action_kind = 'expire_import_candidate'",
                )
                .bind::<Text, _>(resource_id)
                .get_result::<ExpiryAuditEvidence>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn expire_current_candidate(database: &Database) -> Result<(), TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "UPDATE pending_import_candidate \
                     SET expires_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 second') \
                     WHERE singleton = 1",
                )
                .execute(connection)
            })
            .await
            .map_err(|_| TestFailure::FixtureFailed)?
            .map(|_| ())
            .map_err(|_| TestFailure::FixtureFailed)
    }

    async fn vault_record_count(
        database: &Database,
        vault_record_id: &str,
    ) -> Result<i64, TestFailure> {
        let vault_record_id = vault_record_id.to_owned();
        database
            .interact(move |connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM server_vault_records \
                     WHERE vault_record_id = ?",
                )
                .bind::<Text, _>(vault_record_id)
                .get_result::<CountRow>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map(|row| row.value)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn import_audit_count(database: &Database) -> Result<i64, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM audit_events \
                     WHERE action_kind IN \
                     ('create_import_candidate', 'expire_import_candidate')",
                )
                .get_result::<CountRow>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map(|row| row.value)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    async fn persistence_snapshot(database: &Database) -> Result<PersistenceSnapshot, TestFailure> {
        database
            .interact(|connection| {
                diesel::sql_query(
                    "SELECT \
                     (SELECT COUNT(*) FROM pending_import_candidate) AS candidate_count, \
                     (SELECT COUNT(*) FROM server_vault_records) AS vault_count, \
                     (SELECT COUNT(*) FROM audit_events) AS audit_count, \
                     (SELECT candidate_id FROM pending_import_candidate WHERE singleton = 1) \
                       AS candidate_id",
                )
                .get_result::<PersistenceSnapshot>(connection)
            })
            .await
            .map_err(|_| TestFailure::EvidenceFailed)?
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    fn data_version(connection: &mut SqliteConnection) -> Result<i64, TestFailure> {
        diesel::dsl::sql::<BigInt>("PRAGMA data_version")
            .get_result(connection)
            .map_err(|_| TestFailure::EvidenceFailed)
    }

    fn assert_database_files_exclude(path: &Path, canary: &[u8]) -> Result<(), TestFailure> {
        let database_bytes = fs::read(path).map_err(|_| TestFailure::EvidenceFailed)?;
        if contains_bytes(&database_bytes, canary) {
            return Err(TestFailure::DatabaseLeakedSecret);
        }
        let wal_path = PathBuf::from(format!("{}-wal", path.display()));
        if wal_path.exists() {
            let wal_bytes = fs::read(wal_path).map_err(|_| TestFailure::EvidenceFailed)?;
            if contains_bytes(&wal_bytes, canary) {
                return Err(TestFailure::DatabaseLeakedSecret);
            }
        }
        Ok(())
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    fn assert_canonical_uuid_v7(value: &str) -> Result<(), TestFailure> {
        let parsed = Uuid::parse_str(value).map_err(|_| TestFailure::CandidateEvidenceChanged)?;
        if parsed.get_version_num() != 7 || parsed.hyphenated().to_string() != value {
            return Err(TestFailure::CandidateEvidenceChanged);
        }
        Ok(())
    }

    fn correlation_id() -> CorrelationId {
        CorrelationId::from_uuid(Uuid::now_v7())
    }

    struct ImportFixture {
        database: Database,
        database_path: PathBuf,
        master_key_path: PathBuf,
        directory_path: PathBuf,
    }

    impl ImportFixture {
        async fn new() -> Result<Self, TestFailure> {
            let directory_path =
                std::env::temp_dir().join(format!("natsume-server-import-test-{}", Uuid::now_v7()));
            fs::create_dir(&directory_path).map_err(|_| TestFailure::FixtureFailed)?;
            fs::set_permissions(&directory_path, fs::Permissions::from_mode(0o700))
                .map_err(|_| TestFailure::FixtureFailed)?;
            let database_path = directory_path.join("server.sqlite3");
            let master_key_path = directory_path.join("master.key");
            ensure_master_key(&master_key_path).map_err(|_| TestFailure::FixtureFailed)?;
            let database =
                Database::connect_and_migrate(&DatabaseConfig::new(&database_path, true))
                    .await
                    .map_err(|_| TestFailure::FixtureFailed)?;
            Ok(Self {
                database,
                database_path,
                master_key_path,
                directory_path,
            })
        }

        fn observer(&self) -> Result<SqliteConnection, TestFailure> {
            let path = self
                .database_path
                .to_str()
                .ok_or(TestFailure::FixtureFailed)?;
            SqliteConnection::establish(path).map_err(|_| TestFailure::FixtureFailed)
        }
    }

    impl Drop for ImportFixture {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.directory_path);
        }
    }

    #[derive(QueryableByName)]
    struct CandidateEvidence {
        #[diesel(sql_type = Text)]
        candidate_id: String,
        #[diesel(sql_type = Text)]
        expires_at: String,
        #[diesel(sql_type = BigInt)]
        baseline_configuration_revision: i64,
        #[diesel(sql_type = BigInt)]
        baseline_binding_revision: i64,
        #[diesel(sql_type = Binary)]
        preview_token_hash: Vec<u8>,
        #[diesel(sql_type = Text)]
        payload_vault_record_id: String,
        #[diesel(sql_type = Text)]
        redacted_preview_json: String,
        #[diesel(sql_type = Text)]
        record_type: String,
        #[diesel(sql_type = Text)]
        subject_id: String,
        #[diesel(sql_type = Binary)]
        nonce: Vec<u8>,
        #[diesel(sql_type = Binary)]
        ciphertext: Vec<u8>,
        #[diesel(sql_type = BigInt)]
        ttl_valid: i64,
    }

    #[derive(QueryableByName)]
    struct AuditEvidence {
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action_kind: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = Nullable<Text>)]
        resource_id: Option<String>,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = Nullable<Text>)]
        reason_code: Option<String>,
        #[diesel(sql_type = Text)]
        correlation_id: String,
        #[diesel(sql_type = Text)]
        redacted_detail_json: String,
    }

    #[derive(QueryableByName)]
    struct ExpiryAuditEvidence {
        #[diesel(sql_type = BigInt)]
        count: i64,
        #[diesel(sql_type = Text)]
        actor: String,
        #[diesel(sql_type = Text)]
        action_kind: String,
        #[diesel(sql_type = Text)]
        resource_type: String,
        #[diesel(sql_type = Nullable<Text>)]
        resource_id: Option<String>,
        #[diesel(sql_type = Text)]
        result: String,
        #[diesel(sql_type = Nullable<Text>)]
        reason_code: Option<String>,
        #[diesel(sql_type = Text)]
        correlation_id: String,
        #[diesel(sql_type = Text)]
        redacted_detail_json: String,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = BigInt)]
        value: i64,
    }

    #[derive(Debug, PartialEq, Eq, QueryableByName)]
    struct PersistenceSnapshot {
        #[diesel(sql_type = BigInt)]
        candidate_count: i64,
        #[diesel(sql_type = BigInt)]
        vault_count: i64,
        #[diesel(sql_type = BigInt)]
        audit_count: i64,
        #[diesel(sql_type = Nullable<Text>)]
        candidate_id: Option<String>,
    }

    #[derive(Debug, Snafu)]
    enum TestFailure {
        #[snafu(display("the import test fixture failed"))]
        FixtureFailed,
        #[snafu(display("the import candidate could not be created"))]
        CandidateCreationFailed,
        #[snafu(display("an invalid upload was accepted"))]
        InvalidUploadWasAccepted,
        #[snafu(display("an invalid upload reached the vault"))]
        InvalidUploadReachedVault,
        #[snafu(display("an invalid upload wrote data"))]
        InvalidUploadWroteData,
        #[snafu(display("import persistence evidence could not be read"))]
        EvidenceFailed,
        #[snafu(display("the import candidate evidence changed"))]
        CandidateEvidenceChanged,
        #[snafu(display("the preview token hash changed"))]
        PreviewTokenHashChanged,
        #[snafu(display("the staged payload could not be opened"))]
        PayloadOpenFailed,
        #[snafu(display("the staged payload evidence changed"))]
        PayloadEvidenceChanged,
        #[snafu(display("the database contained plaintext or a preview token"))]
        DatabaseLeakedSecret,
        #[snafu(display("the import audit evidence changed"))]
        AuditEvidenceChanged,
        #[snafu(display("a live pending candidate was replaced"))]
        PendingCandidateWasReplaced,
        #[snafu(display("the pending-candidate error changed"))]
        PendingErrorChanged,
        #[snafu(display("a rejected pending upload wrote data"))]
        PendingUploadWroteData,
        #[snafu(display("the expired candidate replacement failed"))]
        ExpiredReplacementFailed,
        #[snafu(display("the expired candidate was not replaced"))]
        ExpiredCandidateWasNotReplaced,
        #[snafu(display("the import expiry audit changed"))]
        ExpiryAuditChanged,
        #[snafu(display("the empty import diff changed"))]
        EmptyDiffChanged,
    }
}
