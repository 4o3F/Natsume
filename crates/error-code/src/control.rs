//! Stable Command and typed device-control error codes.

define_error_codes! {
    /// Public Command and Server-to-Device control error semantics.
    pub enum ControlErrorCode {
        /// A Command ID is not a canonical lowercase hyphenated `UUIDv7`.
        CommandIdInvalid => "COMMAND_ID_INVALID",
        /// The same Command ID names a different canonical request.
        CommandRequestConflict => "COMMAND_REQUEST_CONFLICT",
        /// The negotiated control protocol version is unsupported.
        ProtocolVersionUnsupported => "PROTOCOL_VERSION_UNSUPPORTED",
        /// The accepted control envelope violates the closed typed contract.
        ProtocolInvalidEnvelope => "PROTOCOL_INVALID_ENVELOPE",
        /// The same Command ID reached the Device with a different frozen payload.
        CommandPayloadConflict => "COMMAND_PAYLOAD_CONFLICT",
        /// The frozen Command payload violates its typed contract.
        CommandPayloadInvalid => "COMMAND_PAYLOAD_INVALID",
        /// A required binding, configuration, credential, or Command generation is stale.
        CommandStale => "COMMAND_STALE",
    }
}
