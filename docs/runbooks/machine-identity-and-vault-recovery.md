# Machine identity and vault recovery

## Identity indeterminate

1. Confirm Caddy has not started and `/run/natsume/gateway-tls/ready` is absent.
2. Inspect only typed collector statuses; do not delete identity/vault data.
3. Repair firmware/permission/I/O visibility and restart the daemon.
4. Continue only when the stored ID appears in the current candidate set.

## Identity record missing or corrupt

1. If Client DB, root key, certificate or LKG exists, do not create a replacement identity file.
2. Keep Caddy absent and report `identity_record_missing_or_corrupt`.
3. Confirm whether local state should be preserved for diagnostics.
4. Run an explicit local factory reset, then use ordinary Enrollment.

## Site namespace mismatch

1. Keep all identity-bound state unopened and Caddy absent.
2. Verify the signed `/etc/natsume/site.toml` and trust bundle against deployment records.
3. Restore the correct site material or perform a deliberate full site reprovision.
4. Never rewrite Machine Hardware ID automatically to hide this mismatch.

## Conclusive Machine ID mismatch

1. The daemon keeps Caddy absent and executes the journaled identity-bound reset.
2. It removes Client DB, Client root key, Device/Gateway certificates, installed secret/LKG and the old identity file.
3. It preserves Server endpoint and signed site public material.
4. It writes the current Machine ID, creates a fresh root key and keys/CSRs, then submits ordinary pending Enrollment.
5. Operator handles the Server as a normal new Device; no special clone record is created.

## Identity matched but vault authentication fails

1. Treat as `local_vault_corrupt`; do not infer a new Device.
2. Preserve evidence for diagnostics without copying secret plaintext.
3. Operator performs an explicit local factory reset.
4. Revoke the unusable old certificate, approve `rekey_existing_device` for the same Machine ID, then run explicit state/secret sync. Do not create a duplicate Device row.
