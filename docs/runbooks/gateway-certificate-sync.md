# Gateway certificate issuance during SYNC_STATE

## When this runbook applies

Use this flow when a `SYNC_STATE` target requires local HTTPS and the Device has no Gateway certificate, the SAN/profile differs, the certificate is invalid, or its validity does not reach the target minimum.

## Device-side preparation

1. Confirm the Daemon is connected through mandatory-mTLS QUIC and the matching `SYNC_STATE` is non-terminal and before deadline.
2. Compare the encrypted local certificate against the frozen target. Reuse only when SPKI, DNS SAN, profile, chain and minimum validity all pass.
3. If issuance is needed, generate or select the Gateway private key locally. Persist the encrypted key and request journal before sending anything.
4. Create a CSR and calculate its SPKI SHA-256. Never place the private key in protocol messages, logs or Server storage.
5. Send `GatewayCertificateRequest` with the durable request ID, command ID, target generation, configuration revision, CSR, SPKI hash and nonce.

## Server-side authorization and issuance

1. Resolve the Device exclusively from the authenticated QUIC peer.
2. Load the Command and require `kind=SYNC_STATE`, matching Device, non-terminal state and unexpired deadline.
3. Require exact generation and configuration-revision equality with the frozen command payload.
4. Verify CSR proof, permitted key algorithm and SPKI hash. Ignore CSR SAN/EKU/KeyUsage/CA requests.
5. Derive DNS SAN, certificate profile and validity from the frozen target.
6. Sign with the Origin Issuing Intermediate, then independently parse and verify the resulting leaf.
7. Persist request state, certificate metadata, audit and change event in one transaction before returning `GatewayCertificateResult`.
8. Repeated requests with the same request/command/generation/configuration/SPKI return the same result. Any SPKI mismatch is a conflict.

## Device-side installation

1. Require result request ID, command ID and generation to match the journal.
2. Validate Local Origin chain, SPKI, DNS SAN, EKU=serverAuth, KeyUsage, BasicConstraints CA=false and minimum validity.
3. Store leaf/chain encrypted and persist the terminal request result.
4. Decrypt key/certificate only to `/run/natsume/gateway-tls/<generation>/`, atomically switch the ready marker and start/reload Caddy in visual BLOCKED mode.
5. Continue target application and health checks. Only then may the command report success.

## Failure handling

| Failure | Action |
|---|---|
| QUIC disconnect | reconnect and resend the same durable request before command deadline |
| issuer temporarily unavailable | remain waiting/BLOCKED; retry with bounded backoff |
| command expired or target changed | reject old request; create a new explicit `SYNC_STATE` |
| SPKI conflict | fail command and require operator review/new key/request |
| returned certificate invalid | discard result, keep Caddy absent/BLOCKED, report stable error |
| encrypted vault write fails | do not materialize plaintext or mark request/command complete |

Enrollment and anonymous HTTPS are never valid fallback paths.
