# Packaging

This directory owns the two Debian package manifests, package-owned root files and maintainer scripts. It does not compile Rust, build Web assets or download Caddy; nFPM maps already verified outputs directly into `natsume-server` and `natsume-client`.

The Client package is site-specific at build time. It must receive `SITE_CONFIG`, `CONTROL_CA_CERT` and `LOCAL_ORIGIN_CA_CERT` as verified public inputs. No offline root private key is ever a package input. Debconf asks only for Server IP/port.

## Supply-chain pins

| Tool | Release artifact | Verification |
|---|---|---|
| Caddy `2.11.4` | `caddy_2.11.4_linux_amd64.tar.gz` from the [official GitHub release](https://github.com/caddyserver/caddy/releases/tag/v2.11.4) | `client/caddy.archive.sha256` verifies the archive; `client/caddy.sha256` verifies the extracted binary |
| nFPM `2.47.0` | `nfpm_2.47.0_Linux_x86_64.tar.gz` from the [official GitHub release](https://github.com/goreleaser/nfpm/releases/tag/v2.47.0) | `nfpm.sha256` verifies the host build-tool archive |

`client/caddy.modules` records the required standard modules. The pinned Caddy binary contains only the official standard module set; custom modules are not allowed. Package builds consume a previously verified binary through `CADDY_BIN`; maintainer scripts and runtime services must not download it.

The `linux_amd64` records are the Phase 0 packaging candidate, not target-environment architecture sign-off. They remain `ENV-PROPOSED` until locked CI and target OS evidence exist.

## Site-owned public inputs

Both nFPM manifests require `SITE_CONFIG`, `CONTROL_CA_CERT` and `LOCAL_ORIGIN_CA_CERT`. They are non-secret, site-stable release inputs: the immutable fleet namespace, the public Control Trust Root and the public Local Origin Root. Private root keys are never package inputs. `packaging/site-config.example.toml` documents the shape but is not packaged.

## Session Agent package invariant

The Client package installs exactly one system-wide XDG Autostart entry and no Session Agent systemd user unit. In the current Phase 0 tree the Agent binary establishes only the direct-launch, resident-and-hidden process contract. The selected Slint GUI is implemented in Phase 6; its reviewed reference lives under `docs/reference/session-agent-slint/` and is intentionally not in the current Cargo graph. Package verification must reject a user unit, runtime `.slint` interpretation and external GUI helpers.
