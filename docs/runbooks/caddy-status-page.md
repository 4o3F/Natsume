
# Caddy visual status page

- Caddy starts only after the daemon has passed identity/vault startup and atomically materialized the current Gateway certificate/key plus `/run/natsume/gateway-tls/ready`.
- The bootstrap has no upstream or account credential; its main HTML response is always 503.
- Daemon atomically updates `/run/natsume/gateway-status/status.json` using only restoring, transition-blocked, secret-missing, upstream-unhealthy, recovery-required or unassigned states.
- Desktop Session lock is not a Gateway state and must not reload Caddy.
- Missing/invalid JSON displays a generic blocked state. Never paste error chains, operator text or credentials into the snapshot.
- Before certificate materialization, Session Agent/console status explains Enrollment or vault failure because Caddy is not listening.
