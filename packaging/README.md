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

The Client package installs exactly one system-wide XDG Autostart entry and no Session Agent systemd user unit. The minimal build-time Slint probe (hidden/lazy presentation and the final feature/ELF dependency closure) is desktop-capability evidence: its acceptance criteria are frozen in [the supported platform](../docs/supported-platform.md) and its status is tracked in [Phase 0 status](../docs/gates/phase-0-status.md); Phase 6 owns the complete production GUI. Package verification must reject a user unit, bootstrap/runtime descriptor, runtime `.slint` interpretation and external GUI helpers.

## Endpoint conffile lifecycle

`/etc/natsume/config.toml` is a Debian `config|noreplace` file whose packaged form contains no endpoint. On first configure, postinstall obtains one complete IP-literal/port pair from debconf or one complete paired environment override, validates it through `natsume-device-daemon --print-canonical-endpoint`, and writes atomically. Upgrade/reinstall preserves an existing canonical config unless `dpkg-reconfigure`/`DEBCONF_RECONFIGURE=1` or a paired environment override explicitly replaces it. A partial override, invalid existing config, failed sysusers invocation or failed tmpfiles invocation fails package configuration closed.

`packaging/target-vm/phase0-lifecycle.sh` is the destructive disposable-VM harness for install, reinstall/upgrade, explicit reconfigure, reboot, remove and purge. Shared-runner package-content smoke is not target-OS/G0 lifecycle evidence.

## Hosted package lifecycle

`.github/workflows/package-lifecycle.yml` runs the weekly and pre-release shared-runner install, reinstall, reconfigure, remove and purge lifecycle for both packages. It deliberately has no justfile entry point because the harness is destructive, acknowledgement-gated and restricted to CI or a disposable host. Known limits: reinstall is same-version only — a previous-version upgrade path needs a released predecessor and stays with the target-VM harness — and reboot coverage remains owned by `packaging/target-vm/phase0-lifecycle.sh` on the target VM.
