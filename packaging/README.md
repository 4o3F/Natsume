# Packaging

This directory owns the Server/Client Debian manifests, package-owned runtime
files, image integration inputs and release checks. nFPM packages already-built
Rust/Web outputs and verified Caddy. Both Debs are generic: deployment provisions
the public site configuration and CA certificates after package installation.
No ignored VM experiment is a release input.

| Path | Responsibility |
| --- | --- |
| `server/` | `natsume-server` manifest, runtime rootfs and postinstall |
| `client/` | `natsume-client` manifest, runtime rootfs, fixed PAM/Kiosk entry points, maintainer scripts and supply-chain pins |
| [image/](image/README.md) | Standalone image-builder handoff: all configuration inputs, dependencies, implementation requirements, acceptance criteria and checker |
| `ci-package-smoke.sh` | Build/inspect both real Debs and their runtime/image contracts |
| `check-image-inputs.py` | Verify the source input closure and actual Client Deb image payload |
| `check-maintainer-scripts.py` | Verify non-mutating postinstall file checks in temporary roots |
| `hosted-lifecycle.sh` | Existing acknowledgement-gated disposable-runner lifecycle harness |
| `target-vm/` | Ignored local experiments and evidence; never required by build or installation |

## Client and image handoff

The Client package installs Helper/Daemon/Agent and its fixed runtime integration.
It also installs `image/` at `/usr/share/natsume/image-integration/`. The image
builder reads that installed directory and applies its [manifest](image/manifest.tsv)
after provisioning the official desktop and fixed accounts. Upstream stack merges,
actual UID substitutions, final skel/template generation and offline enablement
remain image build steps; Deb configuration alone does not make a boot-ready image.
The Client Deb contains no deployment configuration or CA certificates. Autoinstall
installs the deployer's complete `config.toml` and matching public CA files under
`/etc/natsume/` before first startup. Package scripts never rewrite or delete them.

The entire `image/` directory can also be archived and handed to an image-builder
project independently. Its README, input contract, implementation guide and
acceptance document contain all required design context; its `check.py` runs
without this repository or Git. The handoff distinguishes the matching Client Deb build dependency from the
complete configuration and CA files supplied by deployment.

The contest role uses Unix user `teams` and `/home/teams`; waiting uses `waiting`
and `/home/waiting`. Protocol/CLI role tokens and `gdm-contest` remain `contest`.
The Client stays installed; this handoff does not depend on an uninstall workflow.
The [image requirements](../docs/gnome-session-image-requirements.zh-CN.md) define
all IMG-01–08 delivery obligations. Release checks and evidence requirements are
in the [image acceptance guide](image/acceptance.md).

## Supply-chain pins

| Tool | Release artifact | Verification |
| --- | --- | --- |
| Caddy `2.11.4` | `caddy_2.11.4_linux_amd64.tar.gz` from the [official release](https://github.com/caddyserver/caddy/releases/tag/v2.11.4) | `client/caddy.archive.sha256` and extracted `client/caddy.sha256` |
| nFPM `2.47.0` | `nfpm_2.47.0_Linux_x86_64.tar.gz` from the [official release](https://github.com/goreleaser/nfpm/releases/tag/v2.47.0) | `nfpm.sha256` |

`client/caddy.modules` lists required official standard modules. Builds consume
verified Caddy through `CADDY_BIN`; maintainer scripts/runtime do not download it.
The `linux_amd64` pins identify the packaging tool/input baseline, not completed
final-image or GPU acceptance.

## Build inputs and checks

Both manifests require `VERSION`, `ARCH` and `RUST_RELEASE_DIR`.
Client additionally requires `CADDY_BIN`; Server requires the built `web/dist`.
Neither manifest consumes `SITE_CONFIG`, `CONTROL_CA_CERT` or `LOCAL_ORIGIN_CA_CERT`.
The matching public site configuration and trust roots are separate deployment
inputs for both packages; no root private key or per-device identity enters either Deb or the image.
`client/config.example.toml` and `server/config.example.toml` describe each side's
complete configuration. They are packaged only under `/usr/share/doc/natsume-{client,server}/`.

`just package-client` / `just package-server` render these variables with
`envsubst` and consume the prebuilt inputs. The Client recipe also checks the
resulting Deb's image payload.
`just ci-packages` downloads and verifies pinned tools, builds production
binaries/Web, produces both real Debs without site inputs, and checks that neither
contains site configuration or CA certificates.
Its output remains `dist/packages/ci/*.deb`; image integration inputs travel
inside the Client Deb and need no third package or separate VM archive.

```sh
python3 packaging/check-image-inputs.py
python3 packaging/check-image-inputs.py --deb /absolute/path/natsume-client.deb
```

The first command verifies that all image files are listed, correctly mapped and
not ignored. The second also compares every input's bytes and metadata with the
actual Deb and rejects missing/extra inputs or accidental direct installation
of image-owned configuration. Package CI runs both checks. It does not execute
image configuration, create users, mount Home or start a desktop on the host.

The Agent has one startup owner: official `org.gnome.Kiosk.Script.service` with
Client's `50-natsume.conf` drop-in, restricted to waiting and executing
`/usr/bin/natsume-session-agent run`. Reject the retired global XDG entry, a
second Agent user service or external GUI helpers. The image adds only the
profile/keyboard/font/scale configuration described in its input set.

## Version releases

Push a tag such as `v2.0.4` to run [release.yml](../.github/workflows/release.yml).
The workflow accepts `vMAJOR.MINOR.PATCH` and SemVer prerelease suffixes such as
`v2.0.4-rc.1` (no build metadata). It reuses the full CI workflow at the tagged
commit; all jobs must pass before GitHub Release publication. Branch/PR CI keeps
its `2.0.4~ci1` package version.

Each release includes `natsume-client_<version>_amd64.deb`,
`natsume-server_<version>_amd64.deb` and `SHA256SUMS`, with automatically generated
release notes. For prereleases, `v2.0.4-rc.1` becomes Debian version `2.0.4~rc.1`,
which sorts before `2.0.4`; the GitHub Release is marked as a prerelease and is
not made latest. Download both packages and the checksum file into one directory
and run `sha256sum --check SHA256SUMS` to verify them.

Publishing uses the workflow's `GITHUB_TOKEN` with `contents: write` only in the
publish job; it needs no site secrets. The tag must already exist remotely, and
an existing release is not overwritten. The weekly hosted lifecycle lane and
target-image acceptance remain separate from this release CI.

## Deployment configuration

Deployment supplies one complete configuration per side:

- Client: `/etc/natsume/config.toml`, containing `[server]` (IP literal and port)
  and `[site]` (Gateway hostname).
- Server: `/etc/natsume-server/config.toml`, containing `[listen]`, `[log]`,
  `[storage]`, `[tls]`, `[site]` (Gateway hostname, certificate expiry and contest
  end), `[trust]` (Control/Local Origin CA paths), and `[runtime]` (the required
  canonical HTTPS `domjudge_origin`).

Both sides also use `/etc/natsume/trust/control-ca.crt` and
`/etc/natsume/trust/local-origin-ca.crt`. Configuration and public certificates
are `root:root`, mode `0644`, with parent directories mode `0755`. The Gateway
hostname and CA files must match the paired deployment. See the Server's
[TLS/Origin issuing material and bootstrap](../server/README.md).

Server `bootstrap` creates or migrates the database schema, then initializes the
first admin and Runtime Config in one business transaction. Service startup synchronizes the
origin from the deployment configuration; edit the configuration and restart the
Server to change the upstream. Package scripts do not initialize business data.

The packages do not own these deployment paths as Debian conffiles. Fresh
install, reinstall, reconfiguration, removal and purge do not generate, rewrite,
change permissions or delete them. Maintainer scripts initialize package users
and directories, report missing inputs and reject existing empty/unreadable
inputs. systemd skips service startup until the configuration and two CA paths
exist; the processes validate their complete configuration when starting.

Generic packages can be preinstalled without deployment inputs. Autoinstall
later installs the complete Client configuration and certificates into the
installed system before first boot. There are no debconf endpoint questions,
endpoint environment overrides, deferral flag or configuration CLI commands.
The deployer owns any later changes to configuration.

The former separate `site.toml` format is no longer read. Move Client identity
and Gateway hostname into `[site]` of its configuration. Move Server Gateway
issuance fields into `[site]`, and CA paths into `[trust]`. Unused schema/fingerprint
metadata and Server-only issuance fields are not Client configuration inputs.
This configuration format change requires updating deployment generation before
upgrading the paired packages; it does not migrate old configuration files.

Image construction enables services against the target root without starting
them. Identity is initialized only on the actual machine's first boot. Upgrade
of Client plus applied image configuration is a planned maintenance operation;
updating `/usr/share/natsume/image-integration` does not silently rewrite vendor
PAM/GDM or a live Home. See the image input README for the exact account/template
handoff and [maintenance requirements](image/integration.md#11-维护与回退) for
whole-system version/state handling. This release uses the current database,
Home-window format and waiting/teams accounts; it provides no pre-refactor migration.

Hosted destructive lifecycle checks remain isolated from image inputs and are
not a requirement to uninstall a deployed workstation Client.
