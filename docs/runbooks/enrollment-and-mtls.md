# Device-only Enrollment and mTLS control

## Preconditions

- `/etc/natsume/config.toml` contains the intended Server IP and numeric port.
- The package-installed Control CA validates the Server certificate IP SAN.
- The Client has completed Machine ID validation and opened or created its encrypted vault.
- No Gateway credential is required for this procedure.

## Procedure

1. Confirm the HTTPS Enrollment listener accepts only challenge, Device CSR, proof, polling and approval data. It must not accept Device Commands, Gateway CSR or Gateway certificate requests.
2. Confirm the normal QUIC listener is separate and requires a Device client certificate.
3. On first start, verify the Daemon locally generates the Device Identity private key and stores it encrypted before submitting the CSR.
4. Review the pending Machine ID, hardware quality, source IP and Device SPKI fingerprint.
5. Approve manually, or verify the active auto-approval policy matched every required subnet, quality, uniqueness, rate and device-count constraint.
6. The Client polls using Device-key proof, receives only the Device leaf/chain, validates the clientAuth profile and stores it encrypted.
7. Confirm the first QUIC session completes mandatory mTLS, certificate SAN/serial/Device checks and `ClientHello` cross-check.
8. Confirm the Gateway key and certificate remain absent. Their absence is expected until an explicit `SYNC_STATE` requires them.

## Conflict handling

- Same Machine ID and same Device SPKI: recover the same pending/issued request idempotently.
- Same Machine ID and different Device SPKI: manual conflict. A confirmed local reset may revoke the old Device certificate and re-key the same Device row; otherwise reject.
- Never create a second Device, merge/split Devices, bypass approval with a token, or use Enrollment to issue a Gateway certificate.

## Verification evidence

- Enrollment DB row has `device_csr_der` and `device_spki_sha256`, and no Gateway CSR/SPKI columns.
- Enrollment response contains Device leaf/chain only.
- Client vault contains Device credential records but no Gateway credential records.
- Authenticated QUIC connection is visible before any Gateway certificate request is possible.
