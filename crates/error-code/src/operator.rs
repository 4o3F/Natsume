//! Stable Panel-to-Server operator error codes.

define_error_codes! {
    /// Public operator and import error semantics.
    pub enum OperatorErrorCode {
        /// The import candidate failed the closed input contract.
        ImportCandidateInvalid => "IMPORT_CANDIDATE_INVALID",
        /// A pending candidate already occupies the singleton staging slot.
        ImportCandidatePending => "IMPORT_CANDIDATE_PENDING",
        /// The referenced candidate can no longer be committed.
        ImportCandidateUnavailable => "IMPORT_CANDIDATE_UNAVAILABLE",
        /// A configuration or binding baseline advanced after preview.
        ImportPreviewStale => "IMPORT_PREVIEW_STALE",
    }
}
