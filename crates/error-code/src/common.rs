//! Stable error codes shared by multiple public boundaries.

define_error_codes! {
    /// Cross-cutting public error semantics.
    pub enum CommonErrorCode {
        /// An internal failure has no safer, more specific public classification.
        InternalError => "INTERNAL_ERROR",
        /// A closed request or argument contract is invalid.
        InvalidRequest => "INVALID_REQUEST",
        /// Authentication failed without disclosing credential state.
        AuthenticationFailed => "AUTHENTICATION_FAILED",
        /// The authenticated caller is not authorized for the operation.
        AuthorizationDenied => "AUTHORIZATION_DENIED",
    }
}
