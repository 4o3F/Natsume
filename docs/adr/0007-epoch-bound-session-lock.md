
# ADR-0007: Desktop-only epoch-bound Session lock

## Decision

`LOCK_SESSION`, `UNLOCK_SESSION` and `TERMINATE_SESSION` are typed durable Commands. Targets freeze `session_instance_id` and `session_epoch`; lock allocates a monotonic `lock_epoch`; unlock must match that epoch and the originating lock command ID.

## Boundary

Completion is the Session Agent desktop gate/logind result. Lock and unlock never load, block, reload or otherwise mutate Caddy. Agent restart reasserts a current lock; a replacement session invalidates stale unlocks.
