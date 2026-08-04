//! Stable local Session boundary error codes.

define_error_codes! {
    /// Public Session Agent and session-action error semantics.
    pub enum SessionErrorCode {
        /// Session epoch, lease, or logind identity is no longer current.
        SessionContextStale => "SESSION_CONTEXT_STALE",
        /// No single eligible managed graphical session is available.
        SessionUnavailable => "SESSION_UNAVAILABLE",
        /// The frozen target image does not support the requested session action.
        SessionActionUnsupported => "SESSION_ACTION_UNSUPPORTED",
        /// The requested action conflicts with the current typed session state.
        SessionStateConflict => "SESSION_STATE_CONFLICT",
    }
}
