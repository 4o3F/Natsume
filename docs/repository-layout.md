
# Repository Layout

## Native ownership

- Cargo owns the Rust graph and `Cargo.lock`.
- pnpm owns the Web graph and `pnpm-lock.yaml`.
- `just` only dispatches native commands.
- nFPM owns final package file mapping and Debian debconf control files, not compilation.

## Top-level policy

Allowed product/contract/verification/release directories: `server`, `client`, `web`, `crates`, `integration-tests`, `packaging`, `docs`.

Do not add generic `apps`, `packages`, `rust`, `tools`, `scripts`, `assets`, `pipeline`, `common`, `utils` or `helpers`. A script follows its owner. A shared crate requires at least two real production consumers and a stable contract.

Package-owned Caddy status assets live under `packaging/client/rootfs/usr/share/natsume/gateway-status`; they are not another Web workspace. Machine-ID validation and vault startup live inside `client/device-daemon`; there is no Identity Guard service or entrypoint. Session lock value types stay in `crates/local-control-api`, and no lock method owns Caddy state. The concise overall Roadmap lives at `docs/implementation-roadmap.md`; detailed Phase 0–7 plans live as separate files under `docs/implementation/`.

## Canonical names

| Directory | package/binary |
|---|---|
| `server` | `natsume-server` |
| `client/device-daemon` | `natsume-device-daemon` |
| `client/privileged-helper` | `natsume-privileged-helper` |
| `client/session-agent` | `natsume-session-agent` |
| `web` | `@natsume/web` |
| `crates/device-protocol` | `natsume-device-protocol` |
| `crates/local-control-api` | `natsume-local-control-api` |
| `crates/machine-identity` | `natsume-machine-identity` |
| `integration-tests` | `natsume-integration-tests` |
