# ADR-0013: Inert target state, explicit state and secret Commands

## Status

Accepted for Natsume V2 v2.5.

## Decision

`DeviceTargetState` is a non-secret, canonical Server record used for preview and drift. It is never pushed automatically. `SYNC_STATE` freezes and applies one target generation/hash.

The command also freezes the local-origin hostname, Gateway certificate profile and minimum certificate validity. When the Device lacks a qualifying Gateway credential, certificate acquisition is a narrow, authenticated subflow of that same `SYNC_STATE`; it is not an Enrollment side effect, a reconnect side effect or an independent generic certificate command.

Password is absent from target state. `SYNC_SECRET` is the only password distribution mechanism and requires a human actor, re-authentication, reason, frozen targets and audit. Automation cannot create it.

## Consequences

- Editing configuration or binding only creates a new target generation; it causes no implicit network effect.
- Reconnect resends only an already-created, non-terminal Command.
- `SYNC_STATE` can remain `running` with `waiting_for_gateway_certificate` while the Origin issuer or QUIC session is temporarily unavailable.
- `SYNC_STATE` succeeds only after its target and required Gateway certificate are validated and durably installed.
- State sync and secret sync remain independently auditable and retryable.
