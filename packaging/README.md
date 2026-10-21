# Packaging

This directory owns the Server, workstation Client and independent DOMjudge-wrapper Debian manifests, package-owned runtime files and prepared assets. nFPM packages Rust/Web outputs and verified runtime assets. All three Debs are generic: deployment provisions configuration, certificates and any source credentials after package installation. No ignored VM experiment is a release input.

| Path | Responsibility |
| --- | --- |
| `server/` | `natsume-server` manifest, runtime rootfs and postinstall |
| `client/` | `natsume-client` manifest, runtime rootfs, fixed PAM/Kiosk entry points and maintainer scripts |
| [print-domjudge/](print-domjudge/README.md) | Independent `natsume-print-domjudge` wrapper, pinned Typst/template/fonts, deployment example and standalone package builder |
| [manifest.json](manifest.json) | Fixed download URLs, versions and SHA-256 values for all build tools and runtime assets |
| [`../tools/assets.ts`](../tools/assets.ts) | `deno task prepare`: dependencies, fixed tools/runtime assets and package-time avatar |
| [`../tools/build.ts`](../tools/build.ts) | Offline compile checks, generated HTTP types and production builds |
| [`../tools/package.ts`](../tools/package.ts) | Render the three nFPM manifests and generate Debs from prepared assets and prebuilt binaries |

## Client environment

Client installs Helper/Daemon/Agent and its package-owned runtime integration. Its [environment requirements](../docs/client-environment.zh-CN.md) are maintained in `docs/`, not included in the Deb. Accounts, desktop configuration, distribution adaptation, Home templates, installer behavior and target-machine acceptance belong to the image project; this repository does not ship image fragments, templates or an image checker. Installing the Deb alone does not produce a configured workstation.

## Build inputs and checks

Use Linux amd64 with Deno from `.deno-version`, the repository Rust toolchain, Node/pnpm for Web, and native build dependencies. Preparation also needs curl, tar and xz; wrapper packaging needs Debian `dpkg-dev` for `dpkg-shlibdeps`. No Python or globally installed nFPM is needed for this build path.

```sh
deno task prepare                         # Download dependencies, tools and runtime assets
deno task check                           # Script, Rust and Web compile checks
deno task build server client print-domjudge
deno task package                         # Write all three Debs to dist/packages/
```

`prepare` is the only download phase. It caches the locked Deno modules, fetches locked Cargo dependencies, performs a frozen pnpm install inside `web/` and prepares fixed nFPM/Caddy/Typst/font/license inputs plus the Client avatar. All fixed URLs, versions and checksums are maintained in `manifest.json`; corrupt fixed-asset caches fail rather than being silently replaced. Prepared tools and downloads live under `target/build/`; the printing runtime remains under `target/print-domjudge/runtime` for the existing opt-in Rust tests. Preparation records dependency/tool input hashes for each target; rerun it when that target's inputs change. Fixed-asset changes invalidate only their consumers, so a printing font change does not invalidate Web or script preparation. Client/workspace preparation also downloads the Linux amd64 Skia binary archive pinned in `manifest.json`, checks its Cargo lockfile version and SHA-256, and later compilation reads it through a local file URL. The Agent enables Skia's `no-compile` feature so missing or incompatible binaries cannot trigger a source-build download fallback. Server/wrapper-only preparation does not need this native asset. Rust, Node, Deno and native OS packages are builder prerequisites, not installed by project scripts.

The subsequent tasks use cached-only Deno modules and offline Cargo. These are toolchain settings, not operating-system network isolation for subprocesses. They do not call prepare, install dependencies or download assets. `package` consumes already built binaries and `web/dist`, parses the existing nFPM manifests, fills explicit inputs and invokes the prepared nFPM. Build only what is required with `deno task prepare client`, `deno task build client`, then `deno task package client`; the [wrapper](print-domjudge/README.md#build-and-package) has the same independent three-step flow without Server or Web. Web-only preparation does not need Cargo; Client/wrapper-only preparation does not need Node.

`VERSION` overrides the Cargo workspace version; `--version` takes precedence over it. All three manifests preserve the supplied Debian version without SemVer normalization, including revisions such as `2.10.3-1`. `--ci` appends `~ci1` only when no explicit version is supplied. `ARCH` is restricted to `amd64`. `RUST_RELEASE_DIR` selects an exact prebuilt directory, with relative paths rooted at the repository; an empty value is rejected. Without it, packaging uses `CARGO_TARGET_DIR/release` or `target/release`. `--output` or `NATSUME_PACKAGE_OUTPUT` selects the output directory. Supplying prebuilt binaries does not trigger compilation; `prepare print-domjudge` with `RUST_RELEASE_DIR` also skips Cargo fetching. The manifests' Caddy/avatar/runtime paths are filled from the prepared assets, not deployment files.

The Client avatar is resolved from the public `4o3f` Gravatar profile during prepare and downloaded as a 512 × 512 PNG. It is baked into the Deb at `/usr/share/natsume/author-avatar.png` (0644), not committed or compiled into Rust. Download/format failures stop preparation without replacing the previous image; its SHA-256 is logged. A later prepare may fetch a changed avatar. Builds, installation scripts and the running Agent never fetch it.

CI follows prepare → production build → package, outputs `dist/packages/ci/*.deb` and retains the artifact name `phase0-deb-content-smoke` for the release consumer. The artifact name is not a package-validation claim. This path does not run the former package-content/image/maintainer checks or real Typst/HTTPS admission tests; the [native Print test commands](print-domjudge/README.md#opt-in-tests) remain opt-in after preparing the printing assets. The Rust job also checks regenerated TypeScript bindings (`deno task api --check`) and the Diesel schema (`deno task schema:check`, requiring Diesel CLI 2.3.12 and rustfmt). API checks generate and format temporary output before comparing with the working file; they do not overwrite it or depend on Git staging. Database schema tests run once with the existing Rust test suite. `check scripts` owns Deno type/format/lint checks; `check rust` runs Clippy without a separate Cargo check. CI also runs the native Deno regression tests in `tools/build_test.ts`. Unit/browser/dependency checks and Release publication remain separate steps.

Third-party license texts are prepared with the assets, not duplicated in Git. Client preparation extracts Caddy's LICENSE and Skia's LICENSE_SKIA from their checksum-verified archives and installs them under `/usr/share/doc/natsume-client/licenses/`. Prebuilt Client preparation also needs the pinned Skia archive for this notice, without invoking Cargo or Node. Typst's LICENSE/NOTICE come from its verified archive, and the font's OFL is a checksum-pinned download; they remain in the wrapper Deb. The printing font is copied to a stable runtime filename independently of its download-cache name. Downloads occur only during prepare. The wrapper separately includes the repository's own `LICENSE`. These packaged texts do not constitute a complete license audit of every transitive dependency.

The packages contain no deployment configuration, CA or credentials. Configuration examples remain under `/usr/share/doc/natsume-{client,server}/`. Building these Debs does not apply image configuration, create desktop users, mount Home or start services.

## Version releases

Push a tag such as `v2.3.2` to run [release.yml](../.github/workflows/release.yml). The workflow accepts `vMAJOR.MINOR.PATCH` and SemVer prerelease suffixes such as `v2.3.2-rc.1` (no build metadata). It reuses the full CI workflow at the tagged commit; all jobs must pass before GitHub Release publication. Branch/PR CI derives `<workspace version>~ci1` from `Cargo.toml` unless the reusable workflow receives an explicit package version.

Each release includes `natsume-client_<version>_amd64.deb`, `natsume-server_<version>_amd64.deb`, `natsume-print-domjudge_<version>_amd64.deb` and `SHA256SUMS`, with automatically generated release notes. Version-specific notes and migration guidance belong in [GitHub Releases](https://github.com/4o3F/Natsume/releases), not separate files in the repository. For prereleases, `v2.3.2-rc.1` becomes Debian version `2.3.2~rc.1`, which sorts before `2.3.2`; the GitHub Release is marked as a prerelease and is not made latest. Download all three packages and the checksum file into one directory and run `sha256sum --check SHA256SUMS` to verify them. To verify only selected downloaded packages, use `sha256sum --ignore-missing --check SHA256SUMS`; the independent wrapper does not require installing the other two packages.

Publishing uses the workflow's `GITHUB_TOKEN` with `contents: write` only in the publish job; it needs no site secrets. The tag must already exist remotely, and an existing release is not overwritten. Target-machine acceptance belongs to the image project, separate from this release CI.

## Deployment configuration

Deployment supplies separate configuration for the paired workstation system and the independently deployed wrapper:

- Client: `/etc/natsume/config.toml`, containing `[server]` (IP literal and port) and `[site]` (Gateway hostname).
- Server: `/etc/natsume-server/config.toml`, containing `[listen]`, `[log]`, `[storage]`, `[tls]`, `[site]` (Gateway hostname, certificate expiry and contest end), `[trust]` (Control/Local Origin CA paths), and `[runtime]` (the required canonical HTTPS `domjudge_origin`).
- DOMjudge wrapper: `/etc/natsume-print-domjudge/config.toml`, containing the remote HTTPS `server_url`, restricted `token_file`, and optional deployment `ca_certificate`; see its [configuration and credentials](print-domjudge/README.md#deployment-owned-configuration-and-credentials).

The workstation Client and Server also use `/etc/natsume/trust/control-ca.crt` and `/etc/natsume/trust/local-origin-ca.crt`. Configuration and public certificates are `root:root`, mode `0644`, with parent directories mode `0755`. The Gateway hostname and CA files must match the paired deployment. See the Server's [TLS/Origin issuing material and bootstrap](../crates/natsume-server/README.md).

Server `bootstrap` creates or migrates the database schema, then initializes the first admin and Runtime Config in one business transaction. Service startup synchronizes the origin from the deployment configuration; edit the configuration and restart the Server to change the upstream. Package scripts do not initialize business data.

The packages do not own these deployment paths as Debian conffiles. Fresh install, reinstall, reconfiguration, removal and purge do not generate, rewrite, change permissions or delete them. Maintainer scripts initialize package users and directories, report missing inputs and reject existing empty/unreadable inputs. systemd skips service startup until the configuration and two CA paths exist; the processes validate their complete configuration when starting.

Generic packages can be preinstalled without deployment inputs. Autoinstall later installs the complete Client configuration and certificates into the installed system before first boot. There are no debconf endpoint questions, endpoint environment overrides, deferral flag or configuration CLI commands. The deployer owns any later changes to configuration.

The former separate `site.toml` format is no longer read. Move Client identity and Gateway hostname into `[site]` of its configuration. Move Server Gateway issuance fields into `[site]`, and CA paths into `[trust]`. Unused schema/fingerprint metadata and Server-only issuance fields are not Client configuration inputs. This configuration format change requires updating deployment generation before upgrading the paired packages; it does not migrate old configuration files.

Identity is initialized only on the actual machine's first boot. Upgrades and recovery must preserve compatible program versions, deployment configuration and existing identity/Home state; see the [environment requirements](../docs/client-environment.zh-CN.md#维护要求). This release provides no pre-refactor migration.
