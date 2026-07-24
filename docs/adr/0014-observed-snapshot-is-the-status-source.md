
# ADR-0014: Observed snapshot is the sole apply-status source

## Decision

Remove `DesiredStateStatus`. `ObservedStateSnapshot` reports received/applied generation, applied hash, apply state, stable error code, installed credential revision and Gateway/Session/Home facts.

This avoids two asynchronously updated status records describing the same Device transition. CommandStatus remains the durable per-command lifecycle; Observed remains current Device fact.
