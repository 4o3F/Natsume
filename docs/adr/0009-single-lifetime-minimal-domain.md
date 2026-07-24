
# ADR-0009: Single-lifetime minimal domain

## Decision

A Server initialization serves one contest only. There is no `Event` entity or phase state. The domain contains SystemConfiguration, AutomationPolicy, immutable Seat, Account/CredentialRevision, SeatAssignment, Device/Binding, target/observed state, Enrollment, Operation/Command and audit.

Natsume does not store team display/organization/category/member metadata. A new contest is a deployment reset, not an in-product event switch.
