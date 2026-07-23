use std::collections::BTreeSet;

use natsume_error_code::{
    ALL_ERROR_CODES, ErrorCode, Redacted, redact_report, to_dbus_name, to_problem_details,
    to_protocol_code,
};
use uuid::Uuid;

#[test]
fn public_registry_maps_every_code_without_detail() {
    let mut protocol_codes = BTreeSet::new();
    let mut dbus_names = BTreeSet::new();

    for code in ALL_ERROR_CODES {
        let problem = to_problem_details(code, Uuid::nil());

        assert_eq!(problem.code(), code.as_str());
        assert_eq!(problem.status(), code.http_status());
        assert!(problem.detail().is_none());
        assert!(protocol_codes.insert(to_protocol_code(code)));
        assert!(dbus_names.insert(to_dbus_name(code)));
    }
}

#[test]
fn public_redaction_types_hide_sensitive_values() {
    let wrapped = Redacted::new("private-value");
    let sanitized = redact_report("vault=/var/lib/natsume/vault.db token=private-value");

    assert_eq!(format!("{wrapped}"), "[REDACTED]");
    assert!(!sanitized.as_str().contains("/var/lib/natsume"));
    assert!(!sanitized.as_str().contains("private-value"));
    assert_eq!(ErrorCode::VaultCorrupt.as_str(), "VAULT_CORRUPT");
}
