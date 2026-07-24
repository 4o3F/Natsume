
# ADR-0010: Immutable Machine ID and delete/re-enroll lifecycle

## Decision

A Device has one UUIDv5 `MachineHardwareId` that cannot be revised. There is no installation instance, machine-ID version, merge, split or reparent operation. Device replacement is unbind old Device, revoke/delete it, enroll the new Device and bind the Seat.

Transient hardware candidates may exist only for local startup or pending Enrollment evidence; the Server does not persist canonical anchor, claim digest or anchor-set hash on Device.
