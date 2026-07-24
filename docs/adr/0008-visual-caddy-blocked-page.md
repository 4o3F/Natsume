
# ADR-0008: Visual Caddy BLOCKED page

## Decision

Use package-owned local HTML/CSS/JS/SVG plus an allowlisted enum JSON snapshot. States are restoring, transition-blocked, secret-missing, upstream-unhealthy, recovery-required and unassigned. Desktop lock is intentionally absent.

## Security

The main response remains HTTP 503. Assets are local; CSP forbids external/default content; JavaScript uses `textContent`; no password, arbitrary errors or remote markup enter the snapshot. Caddy starts only after the daemon materializes the current Gateway key/certificate under `/run`.
