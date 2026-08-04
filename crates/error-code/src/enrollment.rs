//! Stable provisioning and Enrollment error codes.

define_error_codes! {
    /// Public provisioning and Enrollment error semantics.
    pub enum EnrollmentErrorCode {
        /// Enrollment was attempted while provisioning is closed.
        ProvisioningWindowClosed => "PROVISIONING_WINDOW_CLOSED",
        /// The bounded typed Enrollment request is invalid.
        EnrollmentRequestInvalid => "ENROLLMENT_REQUEST_INVALID",
        /// Hardware identity facts conflict and require manual recovery.
        DeviceIdentityConflict => "DEVICE_IDENTITY_CONFLICT",
    }
}
