//! Stable Device identity, Gateway, and secret error codes.

define_error_codes! {
    /// Public Device execution and fail-closed error semantics.
    pub enum DeviceErrorCode {
        /// Identity-bound artifacts exist but current hardware identity is unavailable.
        DeviceIdentityUnavailable => "DEVICE_IDENTITY_UNAVAILABLE",
        /// Current hardware identity differs from the persisted identity.
        DeviceIdentityMismatch => "DEVICE_IDENTITY_MISMATCH",
        /// Identity-bound credential files cannot be read safely.
        DeviceCredentialsUnreadable => "DEVICE_CREDENTIALS_UNREADABLE",
        /// Gateway key or certificate material failed validation.
        GatewayCredentialInvalid => "GATEWAY_CREDENTIAL_INVALID",
        /// Gateway credential material could not be persisted atomically.
        GatewayCredentialInstallFailed => "GATEWAY_CREDENTIAL_INSTALL_FAILED",
        /// Gateway configuration activation could not complete safely.
        GatewayActivationFailed => "GATEWAY_ACTIVATION_FAILED",
        /// The fixed upstream does not satisfy the required TLS policy.
        GatewayUpstreamTlsRequired => "GATEWAY_UPSTREAM_TLS_REQUIRED",
        /// Seat credentials could not be installed atomically.
        SecretInstallFailed => "SECRET_INSTALL_FAILED",
    }
}
