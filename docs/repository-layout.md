# Repository Layout

## Native ownership

- Cargo owns the Rust graph and `Cargo.lock`.
- pnpm owns the Web graph and `pnpm-lock.yaml`.
- `just` only dispatches native commands.
- nFPM owns final package mapping and Debian debconf files, not compilation.

## Top-level policy

Allowed product/contract/verification/release directories are `server`, `client`, `web`, `crates`, `integration-tests`, `packaging`, and `docs`.

Do not add generic `apps`, `packages`, `rust`, `tools`, `scripts`, `assets`, `pipeline`, `common`, `utils` or `helpers`. A script follows its owner. A shared crate requires at least two real production consumers and a stable contract.

`crates/error-code` is the fourth shared production contract. It owns stable strings, explicit HTTP/protocol/D-Bus mappings and report redaction; domain crates retain typed SNAFU errors.

Machine-ID validation and vault startup remain inside `client/device-daemon`; there is no Identity Guard service. Session lock values remain in `crates/local-control-api` and do not own Caddy state.

Session Agent is package-launched only through `/etc/xdg/autostart/org.natsume.SessionAgent.desktop`; there is no systemd user unit. It starts hidden in the authenticated desktop session. The Slint implementation is admitted to the Cargo graph in Phase 6 after the target-desktop probe and lockfile update.

## Canonical names

| Directory | package/binary |
|---|---|
| `server` | `natsume-server` |
| `client/device-daemon` | `natsume-device-daemon` |
| `client/privileged-helper` | `natsume-privileged-helper` |
| `client/session-agent` | `natsume-session-agent` |
| `web` | `@natsume/web` |
| `crates/error-code` | `natsume-error-code` |
| `crates/device-protocol` | `natsume-device-protocol` |
| `crates/local-control-api` | `natsume-local-control-api` |
| `crates/machine-identity` | `natsume-machine-identity` |
| `integration-tests` | `natsume-integration-tests` |
