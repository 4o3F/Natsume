# Explicit state and secret synchronization

## State synchronization

1. Preview the frozen target generation/hash, affected assignment/configuration, local-origin hostname, Gateway certificate profile/minimum validity and disruption.
2. Create `SYNC_STATE`; reconnect alone never sends the target.
3. The Daemon validates the target and checks whether its encrypted Gateway credential satisfies the frozen hostname/profile/validity.
4. When no qualifying credential exists, follow `gateway-certificate-sync.md`. The request must use the same active command, generation and configuration revision over authenticated QUIC.
5. For reassignment, verify the Device blocks the Gateway and clears the old secret before committing the non-secret target.
6. Confirm the Gateway certificate is validated and encrypted before Caddy materialization or target apply completion.
7. Confirm Observed applied generation/hash, Gateway configuration revision/fingerprint/expiry and state apply status.

## Secret synchronization

1. Confirm target/applied assignment match and select the current CredentialRevision.
2. Re-authenticate, enter a reason and freeze targets.
3. Create `SYNC_SECRET`; no automation policy or binding hook may do this.
4. Confirm local encrypted install, Caddy health and Observed installed credential revision.
5. For failure, follow the selected retry/expiry policy; never expose or export the password for manual recovery.

## Prohibited recovery shortcuts

- Do not add Gateway material to Enrollment.
- Do not use a generic certificate-install command.
- Do not create a self-signed or temporary Caddy certificate.
- Do not mark `SYNC_STATE` succeeded while certificate acquisition or validation is incomplete.
