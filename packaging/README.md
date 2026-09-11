# Packaging

This directory owns the Server/Client Debian manifests, package-owned runtime
files, image integration inputs and release checks. nFPM packages already-built
Rust/Web outputs and verified Caddy. The Server Deb also consumes public site
configuration; Client images inject their site configuration and CA certificates
after installing the generic Client Deb. No ignored VM experiment is a release input.

| Path | Responsibility |
| --- | --- |
| `server/` | `natsume-server` manifest, runtime rootfs and postinstall |
| `client/` | `natsume-client` manifest, runtime rootfs, fixed PAM/Kiosk entry points, maintainer scripts and supply-chain pins |
| [image/](image/README.md) | Standalone image-builder handoff: all configuration inputs, dependencies, implementation requirements, acceptance criteria and checker |
| `ci-package-smoke.sh` | Build/inspect both real Debs and their runtime/image contracts |
| `check-image-inputs.py` | Verify the source input closure and actual Client Deb image payload |
| `hosted-lifecycle.sh` | Existing acknowledgement-gated disposable-runner lifecycle harness |
| `target-vm/` | Ignored local experiments and evidence; never required by build or installation |

## Client and image handoff

The Client package installs Helper/Daemon/Agent and its fixed runtime integration.
It also installs `image/` at `/usr/share/natsume/image-integration/`. The image
builder reads that installed directory and applies its [manifest](image/manifest.tsv)
after provisioning the official desktop and fixed accounts. Upstream stack merges,
actual UID substitutions, final skel/template generation and offline enablement
remain image build steps; Deb configuration alone does not make a boot-ready image.
The Client Deb contains no `site.toml` or CA certificates. Image construction
installs the deployer's matching public files under `/etc/natsume/` before first
startup; they remain image-owned across Client reinstall, removal and purge.

The entire `image/` directory can also be archived and handed to an image-builder
project independently. Its README, input contract, implementation guide and
acceptance document contain all required design context; its `check.py` runs
without this repository or Git. The matching Client Deb and site endpoint are
explicit build dependencies described inside the handoff.

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
Client additionally requires `CADDY_BIN`; Server requires `SITE_CONFIG`,
`CONTROL_CA_CERT`, `LOCAL_ORIGIN_CA_CERT` and the built `web/dist`.
The same public site configuration and trust roots are separate Client image
inputs; no root private key or per-device identity enters either Deb or the image.
`site-config.example.toml` describes the public site input and is not packaged.

`just package-client` / `just package-server` render these variables with
`envsubst` and consume the prebuilt inputs. The Client recipe also checks the
resulting Deb's image payload.
`just ci-packages` downloads and verifies pinned tools, builds production
binaries/Web, creates test public site inputs for Server only, produces both real
Debs and checks that Client contains no site configuration or CA certificates.
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

## Endpoint and upgrade contract

`/etc/natsume/config.toml` is a `config|noreplace` conffile whose packaged form
contains no endpoint. First configuration receives a complete IP-literal/port
pair through debconf or `NATSUME_SERVER_IP`/`NATSUME_SERVER_PORT`, validates with
`natsume-device-daemon canonicalize-endpoint`, and writes atomically. An existing
valid endpoint survives reinstall/upgrade unless explicitly reconfigured or
replaced by a complete environment override. Partial overrides, invalid existing
configuration or failed sysusers/tmpfiles fail configuration.

Image builders set `NATSUME_DEFER_ENDPOINT=1` for first-time package installation.
This still initializes sysusers/tmpfiles but leaves no `/etc/natsume/config.toml`
or debconf endpoint values. It rejects an existing endpoint or environment
override. Supply the real IP/port later through autoinstall's `curtin in-target`
command running `dpkg-reconfigure -f noninteractive natsume-client` with the pair
in its environment. Deployment endpoints never enter the cloned image.

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
