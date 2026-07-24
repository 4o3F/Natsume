# ADR-0016: Gateway certificate issuance is bound to authenticated SYNC_STATE

## Status

Accepted for Natsume V2 v2.5.

## Context

A Gateway certificate is meaningful only relative to an explicit local-origin hostname, certificate profile and validity requirement. Those values belong to a frozen target snapshot, not to first-contact Enrollment. The Gateway private key must remain local to the Client, while the Server must prevent a compromised Device from asking the Origin issuer to sign arbitrary names or profiles.

## Decision

The first Gateway key is generated lazily by `natsume-device-daemon` while executing a `SYNC_STATE` command. An existing encrypted Gateway key/certificate may be reused only when its SPKI, DNS SAN, profile, chain and validity satisfy the frozen target.

When issuance is required, the Daemon persists the key and request journal, creates a CSR and sends `GatewayCertificateRequest` on the already authenticated QUIC control stream. The request is bound to:

- authenticated Device identity from the mTLS peer certificate;
- `request_id`;
- `command_id` whose kind is `SYNC_STATE`;
- target generation;
- configuration revision;
- CSR SPKI hash and request nonce.

The Server ignores all CSR-requested SAN, EKU, KeyUsage and CA flags. It derives DNS SAN, profile and validity from the command snapshot, verifies command ownership/state/deadline, signs with the Origin Issuing Intermediate, independently parses the result, persists the request/certificate transaction and returns `GatewayCertificateResult`.

A retry with the same request/command/generation/configuration/SPKI returns the same certificate. The same request identity with a different SPKI is a conflict. There is no anonymous HTTPS recovery route, Enrollment fallback, generic `CertificateIssueRequest`, or `INSTALL_CERTIFICATE` command.

## Failure behavior

- Invalid or expired command: reject with a stable error.
- QUIC disconnect: keep the command/request durable and retry before deadline.
- Issuer temporarily unavailable: report a retryable result and keep Caddy absent/BLOCKED.
- CSR/profile/SPKI conflict: fail closed and require a new explicit sync or operator action.
- Device validates chain, SPKI, SAN, EKU, KeyUsage, BasicConstraints and minimum validity before encrypted installation.

## Consequences

- Device control identity is established before data-plane identity.
- Origin issuance is authorized by an existing Device certificate and a concrete operator-approved command.
- Configuration hostname changes naturally trigger certificate replacement on the next state sync.
- Gateway issuance shares the configuration resource lane and cannot race Caddy activation for the same Device.
