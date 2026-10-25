# Packaging

This directory owns the Linux Debian manifests, Windows portable-package documentation, package-owned runtime files and prepared assets. nFPM packages Linux Rust/Web outputs and verified runtime assets; Windows applications are built natively and packaged as ZIPs. The packages contain no site configuration, certificates, credentials or task records. Deployment supplies these separately. No ignored experiment is a release input.

| Path | Responsibility |
| --- | --- |
| `server/` | `natsume-server` manifest, runtime rootfs and postinstall |
| `client/` | `natsume-client` manifest, runtime rootfs, fixed PAM/Kiosk entry points and maintainer scripts |
| [print-domjudge/](print-domjudge/README.md) | Independent `natsume-print-domjudge` wrapper, pinned Typst/template/fonts, deployment example and standalone package builder |
| [print-client/](print-client/README.md) | Windows print-client portable-package instructions; no installer or deployment data |
| [manifest.json](manifest.json) | Fixed download URLs, versions and SHA-256 values for all build tools and runtime assets |
| [`../tools/assets.ts`](../tools/assets.ts) | `deno task prepare`: dependencies, fixed tools/runtime assets and package-time avatar |
| [`../tools/build.ts`](../tools/build.ts) | Offline compile checks, generated HTTP types and production builds |
| [`../tools/process.ts`](../tools/process.ts) | Package definitions, delivery platforms, Rust check platforms and preparation fingerprints |
| [`../tools/package.ts`](../tools/package.ts) | Generate native Debs or Windows ZIPs from prepared assets and prebuilt binaries |
| [`../tools/windows.ts`](../tools/windows.ts) | Stage declared Windows executables, documentation, metadata and checksums; verify ZIP round trips |
| [`../tools/native-deps.ts`](../tools/native-deps.ts) | Install declared Ubuntu builder prerequisites without an inline shell script |
| [`../tools/release.ts`](../tools/release.ts) | Validate tag versions, verify expected packages and publish GitHub Releases |
| [`../.github/actions/prepare-build/action.yml`](../.github/actions/prepare-build/action.yml) | Shared tool setup, Rust compilation caching and unconditional input preparation |

## Client environment

Client installs Helper/Daemon/Agent and its package-owned runtime integration. Its [environment requirements](../docs/client-environment.zh-CN.md) are maintained in `docs/`, not included in the Deb. Accounts, desktop configuration, distribution adaptation, Home templates, installer behavior and target-machine acceptance belong to the image project; this repository does not ship image fragments, templates or an image checker. Installing the Deb alone does not produce a configured workstation.

## Build inputs and checks

Use Linux amd64 with stable Deno at or above `.deno-version` within the same major, the repository Rust toolchain, Node/pnpm for Web, and native build dependencies. CI uses the exact Deno baseline; local newer stable releases in that major do not require downgrading. Preparation also needs curl, tar and xz; wrapper packaging needs Debian `dpkg-dev` for `dpkg-shlibdeps`. No Python or globally installed nFPM is needed for this build path.

```sh
deno task prepare                         # Download dependencies, tools and runtime assets
deno task check                           # Script, Rust and Web compile checks
deno task build server client print-domjudge
deno task package                         # Write all three Debs to dist/packages/
```

`prepare` is the only download phase. It caches the locked Deno modules, fetches locked Cargo dependencies, performs a frozen pnpm install inside `web/` and prepares fixed nFPM/Caddy/Typst/font/license inputs plus the Client avatar. All fixed URLs, versions and checksums are maintained in `manifest.json`; corrupt fixed-asset caches fail rather than being silently replaced. Prepared tools and downloads live under `target/build/`; the printing runtime remains under `target/print-domjudge/runtime` for the existing opt-in Rust tests. Preparation records dependency/tool input hashes for each target; rerun it when that target's inputs change. Fixed-asset changes invalidate only their consumers, so a printing font change does not invalidate Web or script preparation. Client/workspace preparation also downloads the Linux amd64 Skia binary archive pinned in `manifest.json`, checks its Cargo lockfile version and SHA-256, and later compilation reads it through a local file URL. The Agent enables Skia's `no-compile` feature so missing or incompatible binaries cannot trigger a source-build download fallback. Server/wrapper-only preparation does not need this native asset. Rust, Node, Deno and native OS packages are local builder prerequisites. Local preparation does not install toolchains or system packages; the CI preparation action supplies its declared prerequisites before invoking these tasks.

The subsequent tasks use cached-only Deno modules and offline Cargo. These are toolchain settings, not operating-system network isolation for subprocesses. They do not call prepare, install dependencies or download assets. `package` consumes already built binaries and `web/dist`, parses the existing nFPM manifests, fills explicit inputs and invokes the prepared nFPM. Build only what is required with `deno task prepare client`, `deno task build client`, then `deno task package client`; the [wrapper](print-domjudge/README.md#build-and-package) has the same independent three-step flow without Server or Web. Web-only preparation does not need Cargo; Client/wrapper-only preparation does not need Node.

`VERSION` overrides the Cargo workspace version; `--version` takes precedence over it. All three manifests preserve the supplied Debian version without SemVer normalization, including revisions such as `2.10.3-1`. `--ci` appends `~ci1` only when no explicit version is supplied. `ARCH` is restricted to `amd64`. `RUST_RELEASE_DIR` selects an exact prebuilt directory, with relative paths rooted at the repository; an empty value is rejected. Without it, packaging uses `CARGO_TARGET_DIR/release` or `target/release`. `--output` or `NATSUME_PACKAGE_OUTPUT` selects the output directory. Supplying prebuilt binaries does not trigger compilation; `prepare print-domjudge` with `RUST_RELEASE_DIR` also skips Cargo fetching. The manifests' Caddy/avatar/runtime paths are filled from the prepared assets, not deployment files.

The Client avatar is resolved from the public `4o3f` Gravatar profile during prepare and downloaded as a 512 × 512 PNG. It is baked into the Deb at `/usr/share/natsume/author-avatar.png` (0644), not committed or compiled into Rust. Download/format failures stop preparation without replacing the previous image; its SHA-256 is logged. A later prepare may fetch a changed avatar. Builds, installation scripts and the running Agent never fetch it.

CI declares one native GitHub Actions `strategy.matrix.include` directly in `.github/workflows/ci.yml`: Server, workstation Client and DOMjudge wrapper run separately on Ubuntu; Print Client runs on Windows. Shared and Web checks run in parallel first; both must succeed before the module matrix starts. A failed or cancelled prerequisite prevents module builds and packaging, and a failed matrix job cancels remaining unfinished module jobs. Each module performs its own prepare, compile checks, unit/integration/documentation tests, release build and native packaging, then uploads `package-<module>-<runner.os>-amd64` from `dist/packages/ci/`. Server owns generated TypeScript and Diesel schema checks, using pinned Diesel CLI 2.3.12 and rustfmt. Client checks include its protocol and IPC library crates, not only its three applications. Shared checks retain formatting, dependency policy, build-script tests and the print client's Linux shared regressions; dedicated Web checks own Web type checks, unit tests and browser coverage; Server builds production Web assets but does not repeat those checks. There is no additional aggregate Rust compile/test job. API checks compare temporary generated output without modifying the working file. The former package-content/image/maintainer suite and real Typst/HTTPS tests are not implied by packaging; the [native Print commands](print-domjudge/README.md#opt-in-tests) remain opt-in.

Third-party license texts are prepared with the assets, not duplicated in Git. Client preparation extracts Caddy's LICENSE and Skia's LICENSE_SKIA from their checksum-verified archives and installs them under `/usr/share/doc/natsume-client/licenses/`. Prebuilt Client preparation also needs the pinned Skia archive for this notice, without invoking Cargo or Node. Typst's LICENSE/NOTICE come from its verified archive, and the font's OFL is a checksum-pinned download; they remain in the wrapper Deb. The printing font is copied to a stable runtime filename independently of its download-cache name. Downloads occur only during prepare. The wrapper separately includes the repository's own `LICENSE`. These packaged texts do not constitute a complete license audit of every transitive dependency.

The packages contain no deployment configuration, CA or credentials. Configuration examples remain under `/usr/share/doc/natsume-{client,server}/`. Building these Debs does not apply image configuration, create desktop users, mount Home or start services.

## Windows portable packages

Use native Windows x64 with the repository Rust toolchain, MSVC build tools, Windows SDK, supported Deno and PowerShell 7 (`pwsh`). Node, pnpm, nFPM and Linux workstation assets are not required for `print-client`. Deno and PowerShell are builder tools, not application runtime dependencies. Run each command only after the previous one succeeds:

```powershell
deno task prepare print-client
if ($LASTEXITCODE -ne 0) { throw 'Preparation failed; do not continue.' }
deno task check print-client
if ($LASTEXITCODE -ne 0) { throw 'Checks failed.' }
deno task test print-client
if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
deno task build print-client
if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
deno task package print-client --ci
if ($LASTEXITCODE -ne 0) { throw 'Packaging failed.' }
```

Preparation fetches locked Cargo and Deno inputs. Check, test and release build use frozen dependencies; package never compiles or downloads. The output is `dist/packages/natsume-print-client_<version>_windows_amd64.zip`. Without an explicit version, Windows uses the Cargo workspace version, with `-ci.1` added for `--ci`; explicit `--version` or `VERSION` must be SemVer. Linux retains its existing Debian version rules, including `~ci1`. `RUST_RELEASE_DIR`, `CARGO_TARGET_DIR`, `--output` and `NATSUME_PACKAGE_OUTPUT` work as for Linux. Prebuilt preparation does not fetch Rust dependencies.

Each ZIP contains only its declared amd64 PE executables, public README, project `LICENSE`, `package.json` and `SHA256SUMS`. No wildcard copies a build directory. Package metadata records name, version, Windows platform, amd64 architecture and executable names. Packaging creates the ZIP in a temporary directory, extracts it and compares every file and hash before publishing the archive. The package version is an artifact label and does not rewrite the binary's compiled Cargo version. The included project license is not a complete transitive-dependency license audit. The ZIP does not install runtime libraries, drivers or trust roots; clean-system runtime and hardware acceptance remain separate.

The package definitions in `tools/process.ts` separately record delivery platform and Rust check platforms. Linux `all` retains script, Rust, Web and Linux package targets; Windows `all` selects Windows packages only. Explicit cross-platform package selections fail before preparation. Rust checks retain the print client's Linux shared tests, exclude packages that explicitly do not support Linux, and select native package crates on Windows. `deno task check rust` and `deno task test rust` are also used by the corresponding hooks, so CI and local checks use the same platform rules.

Every delivery module uses the same matrix and preparation action. The workflow directly declares its module, runner and native prerequisites; package definitions own supported platforms, crates, binaries, formats and build inputs. Regression tests check that every package has exactly one matrix entry on the correct runner. Adding another Rust Windows application requires its source, target definition, public package README and one matrix entry, not duplicated job steps or another Release asset loop. Declare its actual Rust check platforms rather than adding empty Linux implementations. `deno task check shared` and `deno task test shared` verify supported cross-platform crates without preparing Linux delivery assets. Additional packaging formats or runtime assets require their own explicit implementation; this is not a plugin framework.

## CI preparation and Rust caching

The shared preparation action installs declared tools and native prerequisites, enables only the Rust setup action's compilation cache, then runs `prepare` unconditionally. Workflow YAML directly declares the native matrix, job dependencies and conditional packaging steps and calls existing build tasks without a generic CI command router. Only prerequisite installation and release validation/publication need dedicated Deno scripts. Dependency-free helpers in `tools/entry.ts` allow native prerequisites to be installed before downloaded modules are prepared.

Rust cache keys distinguish OS, module and declared native runner and incorporate the native-input manifest, compiler/toolchain, Cargo manifests/lockfiles and compilation environment. Compiled dependencies live under `target/cargo`, separate from preparation state. Workspace applications and incremental output are not persisted by the cache action. Only successful `v2` push jobs save this cache; pull requests and tag releases may restore it. Cache eviction or a miss affects performance, not correctness, and hits never skip preparation or validation.

Deno modules, pnpm dependencies and fixed assets are prepared in each CI job without separate cache restore/save steps or cache-only matrix fields. Their normal local stores remain available within that job, and checks/build/package still consume prepared inputs. Prepared markers, extracted runtime directories, avatars, final packages, credentials and business data are not persisted as CI caches. Frozen dependency installation and fixed-asset checksum checks remain required; corrupt fixed inputs fail rather than being bypassed. Hosted cache behavior and performance require observation in actual CI.

## Version releases

Push a tag such as `v2.3.2` to run [release.yml](../.github/workflows/release.yml). The workflow accepts `vMAJOR.MINOR.PATCH` and SemVer prerelease suffixes such as `v2.3.2-rc.1` (no build metadata). It reuses the full CI workflow at the tagged commit; all jobs must pass before GitHub Release publication. Linux branch/PR CI derives `<workspace version>~ci1` from `Cargo.toml`; Windows uses `<workspace version>-ci.1` unless the reusable workflow receives the corresponding explicit version.

Each release includes `natsume-client_<version>_amd64.deb`, `natsume-server_<version>_amd64.deb`, `natsume-print-domjudge_<version>_amd64.deb`, the ZIP for every Windows package target, and a combined `SHA256SUMS`, with automatically generated release notes. Version-specific notes and migration guidance belong in [GitHub Releases](https://github.com/4o3F/Natsume/releases), not separate files in the repository. For prereleases, `v2.3.2-rc.1` becomes Debian version `2.3.2~rc.1`, which sorts before `2.3.2`; Windows ZIPs retain `2.3.2-rc.1`. The reusable CI workflow accepts separate `package_version` and `release_version` inputs for these formats. Publication reads expected deliveries from the package definitions rather than a matrix-generation job output, and checks Debian metadata and each expected Windows ZIP's integrity, name, version, platform and architecture before including it. Missing or mismatched Windows output prevents publication. The GitHub Release is marked as a prerelease and is not made latest. Download all release packages and the checksum file into one directory and run `sha256sum --check SHA256SUMS` to verify them. To verify only selected downloaded packages, use `sha256sum --ignore-missing --check SHA256SUMS`; the independent wrapper does not require installing the other two packages.

Publishing uses the workflow's `GITHUB_TOKEN` with `contents: write` only in the publish job; it needs no site secrets. The tag must already exist remotely, and an existing release is not overwritten. Target-machine acceptance belongs to the image project, separate from this release CI.

## Deployment configuration

Deployment supplies separate configuration for the paired workstation system and the independently deployed wrapper:

- Client: `/etc/natsume/config.toml`, containing `[server]` (IP literal and port) and `[site]` (Gateway hostname).
- Server: `/etc/natsume-server/config.toml`, containing `[listen]`, `[log]`, `[storage]`, `[tls]`, `[site]` (Gateway hostname, certificate expiry and contest end), `[trust]` (Control/Local Origin CA paths), and `[runtime]` (the required canonical HTTPS `domjudge_origin`).
- DOMjudge wrapper: `--config <PATH>`, defaulting to `/etc/natsume-print-domjudge/config.toml`, containing the remote HTTPS `server_url`, restricted `token_file`, and optional deployment `ca_certificate`; see its [configuration and credentials](print-domjudge/README.md#deployment-owned-configuration-and-credentials).

The workstation Client and Server also use `/etc/natsume/trust/control-ca.crt` and `/etc/natsume/trust/local-origin-ca.crt`. Configuration and public certificates are `root:root`, mode `0644`, with parent directories mode `0755`. The Gateway hostname and CA files must match the paired deployment. See the Server's [TLS/Origin issuing material and bootstrap](../crates/natsume-server/README.md).

Server `bootstrap` creates or migrates the database schema, then initializes the first admin and Runtime Config in one business transaction. Service startup synchronizes the origin from the deployment configuration; edit the configuration and restart the Server to change the upstream. Package scripts do not initialize business data.

The packages do not own these deployment paths as Debian conffiles. Fresh install, reinstall, reconfiguration, removal and purge do not generate, rewrite, change permissions or delete them. Maintainer scripts initialize package users and directories, report missing inputs and reject existing empty/unreadable inputs. systemd skips service startup until the configuration and two CA paths exist; the processes validate their complete configuration when starting.

Generic packages can be preinstalled without deployment inputs. Autoinstall later installs the complete Client configuration and certificates into the installed system before first boot. There are no debconf endpoint questions, endpoint environment overrides, deferral flag or configuration CLI commands. The deployer owns any later changes to configuration.

The former separate `site.toml` format is no longer read. Move Client identity and Gateway hostname into `[site]` of its configuration. Move Server Gateway issuance fields into `[site]`, and CA paths into `[trust]`. Unused schema/fingerprint metadata and Server-only issuance fields are not Client configuration inputs. This configuration format change requires updating deployment generation before upgrading the paired packages; it does not migrate old configuration files.

Identity is initialized only on the actual machine's first boot. Upgrades and recovery must preserve compatible program versions, deployment configuration and existing identity/Home state; see the [environment requirements](../docs/client-environment.zh-CN.md#维护要求). This release provides no pre-refactor migration.
