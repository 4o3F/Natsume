# ADR-0012: Device-only Server-auth Enrollment followed by mandatory-mTLS QUIC

## Status

Accepted for Natsume V2 v2.5.

## Context

A newly installed Client has no Device certificate, so it cannot enter the normal mutually authenticated QUIC control listener. At that point it also has no authoritative `DeviceTargetState`: the local origin hostname, Gateway certificate profile and minimum validity requirement are not yet frozen by a `SYNC_STATE` command.

Issuing a Gateway certificate during Enrollment would therefore mix two trust transitions:

1. approving the Daemon as a control-plane Device;
2. provisioning a data-plane identity for a configuration that may not exist yet.

It would also give the anonymous Enrollment surface a second certificate-issuing capability that is not needed to establish control connectivity.

## Decision

The Client first uses HTTPS that authenticates only the Server through the package-installed Control CA and configured Server IP SAN. Enrollment is manually approved or approved by an explicit bounded policy; there is no one-time token.

Enrollment accepts exactly one locally generated CSR: the Device Identity CSR. Approval signs and returns only the `clientAuth` Device leaf and chain used by `natsume-device-daemon` for QUIC. Enrollment request, persistence schema and poll result contain no Gateway CSR, Gateway SPKI or Gateway certificate.

After issuance, normal Quinn/QUIC control uses a separate rustls `ServerConfig` that requires a valid Device client certificate. 0-RTT is disabled. Quinn/rustls owns TLS 1.3 and QUIC packet protection; Natsume validates the authenticated peer identity and application protocol.

Gateway key generation and certificate issuance are deferred to ADR-0016 and can occur only inside an authenticated `SYNC_STATE` execution.

## Server control certificate provisioning

The Server generates its control private key directly inside the encrypted vault and exports only a CSR. The offline site Control Trust Root signs an IP-SAN leaf, which is imported only after SPKI/profile validation. Package installation never embeds or downloads an offline root private key.

A stable Local Origin Root separately signs the Server-side Origin Issuing Intermediate. That issuer may be unavailable while Enrollment and normal Device mTLS remain healthy; it is required only when a `SYNC_STATE` needs a new Gateway leaf.

## Consequences

- A successful Enrollment proves only that the Daemon can authenticate to the control plane.
- The Client may have a Device key/certificate while Gateway key/certificate are completely absent.
- Phase 3 owns Device Enrollment and first mTLS connection; Phase 5 owns the first Gateway key/CSR/certificate path.
- Enrollment compromise cannot directly mint local-origin server certificates.
- Caddy remains absent or BLOCKED until an explicit state sync obtains a valid Gateway certificate.
