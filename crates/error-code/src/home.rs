//! Stable local Home lifecycle error codes.

define_error_codes! {
    /// Public Home prepare and cleanup error semantics.
    pub enum HomeErrorCode {
        /// The requested Home epoch is no longer current.
        HomeEpochStale => "HOME_EPOCH_STALE",
        /// Home prepare or cleanup cannot complete with proven safety.
        HomeOperationFailed => "HOME_OPERATION_FAILED",
    }
}
