# ADR-0006: Daemon-integrated Machine ID startup check

## Decision

Persist exactly one immutable Machine Hardware ID in an independent atomic file together with schema version, site namespace and checksum. At every daemon start, inventory local identity-bound artifacts before opening the encrypted vault:

- all artifacts absent: clean first start;
- identity record missing/corrupt while DB, root key, certificate or LKG exists: fail closed;
- stored namespace differs from deployment namespace: fail closed;
- stored ID present in current candidates: continue;
- evidence temporarily unavailable: fail closed and retry without deletion;
- complete contradictory evidence: delete identity-bound local state and return to the same first-start path as a clean installation.

There is no installation instance, identity alias graph, separate Identity Guard service, special clone reason or special enrollment path.

## Recovery

Vault authentication failure after an identity match is `local_vault_corrupt`, not evidence of a new device. Identity-record loss and site-namespace mismatch are also not first-start states. Each requires an explicit local factory-reset or site reprovisioning workflow.
