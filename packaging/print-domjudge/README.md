# DOMjudge print admission wrapper

This is the independent Linux amd64 Ubuntu/Debian delivery of `natsume-print-domjudge`. The authoritative behavior and wire contract are [architecture section 23.2.1](https://github.com/4o3F/Natsume/blob/v2/docs/architecture.md#2321-i1-受理协议与独立交付). It does not depend on `natsume-server`, workstation daemons, Device IPC, a local database, a printing service or shared server files. It registers over HTTPS, generates a fixed A4 black-text PDF, then uploads it; only a confirmed `ready` fact means success. Success means queued, not physically printed. Windows execution, physical printing and lifecycle acceptance are not delivered by this package.

## Build and package

Use a Linux amd64 builder with Deno from the repository `.deno-version`, the repository Rust toolchain and Cargo lockfile, trusted HTTPS roots, curl, tar/xz and Debian `dpkg-dev` (`dpkg-shlibdeps`). Node, Web and Python are not required for this independent build path. [`../manifest.json`](../manifest.json) pins the Typst 0.14.2 musl archive, executable, Noto Sans Mono CJK SC Sans2.004 font and font license by SHA-256. The font includes Latin and Chinese glyphs. Prepare extracts Typst's Apache-2.0 license/third-party notices from its verified archive and downloads the pinned SIL OFL 1.1 font license. These are not duplicated in Git; they are included in the Deb with the wrapper's AGPL license to preserve redistribution notices. Downloads are build artifacts; checksums identify reviewed bytes, not independent upstream signature verification.

```sh
deno task prepare print-domjudge
deno task build print-domjudge
deno task package print-domjudge
# Inspect the actual filename emitted by nFPM:
dpkg-deb --info dist/packages/natsume-print-domjudge_*.deb
dpkg-deb --contents dist/packages/natsume-print-domjudge_*.deb
```

Preparation downloads dependencies, fixed nFPM, Typst, fonts and licenses. Fixed downloads are cached under `target/build/downloads` and rechecked on prepare; corrupt cache entries require explicit removal. The prepared runtime remains under `target/print-domjudge/runtime`. Prepare must succeed before build or package; neither later step downloads or installs anything. Build compiles only the wrapper, never Server or Web. Package consumes the resulting binary without compiling it.

For an existing binary, set `RUST_RELEASE_DIR` before prepare and package, then omit build. This preparation mode does not invoke Cargo; packaging uses exactly the selected binary, with relative directories resolved from the repository root and empty values rejected. `VERSION` or `--version` overrides the Cargo workspace version without normalization. `ARCH` defaults to `amd64` and other values are rejected. `--output` selects the destination; output is `natsume-print-domjudge_${VERSION}_amd64.deb`, retaining CI/prerelease suffixes.

Packaging runs `dpkg-shlibdeps` on both wrapper and bundled Typst to generate native library requirements. Typst is currently static musl and may cause an informational no-dynamic-symbols warning. Build on the oldest supported deployment distribution and validate there; generated ABI requirements do not establish universal Debian compatibility. There are no maintainer scripts, services, credentials, installed site configuration or runtime downloads.

Package CI uses the same prepare/build/package flow, without a separate package-validation suite. The real-Typst and Print HTTPS tests remain opt-in after preparation.

## Opt-in tests

Run these commands from the repository root. Poppler `pdftotext`/`pdfinfo` and Python are required for the real rendering tests and existing test-only compiler fixtures; they are not requirements of the build scripts or installed application.

```sh
deno task prepare print-domjudge
cargo test --frozen -p natsume-print-domjudge
cargo test --frozen -p natsume-print-domjudge --lib -- --ignored
cargo test --frozen -p natsume-server --lib print
cargo test --frozen -p natsume-server --lib http::tests::print_domjudge -- --ignored
```

The tests exercise multilingual/long-line rendering, deterministic output, actual TLS admission, replay, response loss, database reopen, trust rejection and revocation. They do not execute deployed DOMjudge/PHP, Windows or a physical printer. Installation, upgrades, DOMjudge integration and production concurrency still require their own target-environment acceptance.

## Deployment-owned configuration and credentials

Install the package on the DOMjudge PHP execution host. An administrator creates `/etc/natsume-print-domjudge/config.toml` using `config.example.toml`, supplies the dedicated source token obtained through the Server's administrator-only source creation endpoint, and optionally installs the deployment CA certificate. No token is generated, embedded or replaced by this package. The Server can be on another machine; there is no shared-directory requirement. Use a strict HTTPS origin with no credentials, API path, query or fragment. With an explicit CA file, that CA bundle is the exclusive trust root set; otherwise system certificate trust is used. Hostname/SAN verification is always enabled, redirects are refused and proxy environment variables are not used. There is no insecure mode or endpoint environment override.

Use root-owned, non-group/world-writable configuration/CA files and protected parent directories. Give only the DOMjudge execution UID permission to read the token: for example, a token owned by that UID with mode `0400` or `0600`, inside a root-controlled traversable directory. Root ownership is also accepted when the executing process can read the file. A root-owned mode-0600 token cannot be read by an unprivileged PHP worker. Group/world-readable token files, unsafe owners, group/world-writable configuration/CA files, final-component symlinks and non-regular files are rejected. One trailing LF is accepted in the token file; the token itself must exactly match `nps1.<canonical UUIDv7>.<64 lowercase hex digits>`. Do not put tokens in CLI arguments, environment variables, DOMjudge configuration, logs or screenshots. Revoke and replace compromised source tokens through the Server's source-management boundary.

The executable accepts `--config <PATH>` to select the deployment configuration file; when omitted, it uses `/etc/natsume-print-domjudge/config.toml`. An explicitly selected file that cannot be loaded causes an error without falling back to the default file. The public Rust `Config::load(path)` and `run(config, runtime, input, started, cancellation)` functions also let integration tests supply private fixture paths. Packaged runtime paths remain fixed; there is no configurable template, compiler path or font search path.

## DOMjudge 9.0.x configuration

The official [9.0 printing manual](https://www.domjudge.org/docs/manual/9.0/config-advanced.html#printing) and [configuration reference](https://www.domjudge.org/docs/manual/9.0/configuration-reference.html#print-command) describe `print_command`. Configure it as follows:

```text
/usr/bin/natsume-print-domjudge --file=[file] --account=[username] --original=[original]
```

The version-pinned [DOMjudge 9.0.1 implementation](https://github.com/DOMjudge/domjudge/blob/9.0.1/webapp/src/Service/DOMJudgeService.php#L737-L800) substitutes `[file]`, `[original]`, `[language]`, `[username]`, `[teamname]`, `[teamid]` and `[location]` using `escapeshellarg`, invokes the command, collects stdout and treats exit code zero as success. Do not add another quoting layer around the placeholders. The equals-sign form also keeps a leading `-` in a username or filename inside its argument value. DOMjudge itself uses a shell to launch its configured command; the wrapper never launches a shell, evaluates source text or constructs executable Typst from user input. Pass `[username]` exactly, not `[teamid]`, team name or location. The Server resolves the existing Natsume account and account ID; the wrapper does not normalize case or whitespace or guess a mapping. Jury accounts without an existing mapped account will be rejected; this is not a general-purpose jury printing gateway.

The [9.0.1 team result template](https://github.com/DOMjudge/domjudge/blob/9.0.1/webapp/templates/team/print_result.html.twig) displays escaped stdout in a `<pre>` and also displays the fixed success message `File has been printed` on zero exit. The wrapper cannot make that stock message true: its own stdout explicitly says `已入队，尚未打印完成`, followed by admission-time ahead task/page counts, in-progress count, registration display snapshots and request ID. Application output uses tracing events written to stdout, with `INFO` for success/help and `ERROR` for failures, without timestamps, targets or ANSI colors; CLI argument and configuration errors use the same initialized subscriber. Display fields are JSON-quoted to escape terminal control characters. Staff must explain this limitation and check the exact deployed UI/version. No mandatory DOMjudge page patch is included. Nonzero stdout does not imply that an ambiguous request was never accepted.

## Invocation and controlled retries

```text
natsume-print-domjudge --file=/absolute/source --account=exact-username --original=source.cpp
natsume-print-domjudge --config=/srv/domjudge/print.toml --file=/absolute/source --account=exact-username --original=source.cpp
natsume-print-domjudge --file=/absolute/source --account=exact-username --original=source.cpp --request-id=01900000-0000-7000-8000-000000000001
```

The source must be a regular UTF-8 file of at most 256 KiB; the final path component is opened with no-follow and nonblocking semantics before checking its file type. Special files, directories and symbolic links are not read. The original filename is display data only: at most 1024 UTF-8 bytes and no path separators, NUL or control characters. Source and registration snapshots are written as text/JSON into a newly created mode-0700 temporary root. Data is never interpolated into Typst code. The fixed template wraps even unbroken long lines, preserves their final characters and renders indentation (tabs as four spaces). Typst runs single-threaded with both system and embedded font discovery disabled, using only the package's font directory. Its fixed metadata and registration snapshots make intentional same-ID retries byte-stable with the same package and input. Changing template/font/compiler versions between preparing-state retries is not promised to produce identical bytes; a ready registration is returned directly without regeneration.

Compilation has a two-second budget. The five-second whole invocation budget starts before CLI/configuration processing and is shared by registration, compilation, upload and outcome verification. The compiler is killed and explicitly reaped on compile timeout, total deadline or handled SIGINT/SIGTERM cancellation; ephemeral input/output files are deleted on exit. Child stdout/stderr go to the null device, not user output or an unbounded memory buffer. HTTP response buffering is capped at 64 KiB and PDFs at 8 MiB. Local regular-file reads and process reaping depend on the operating system making progress; this is not a hard real-time scheduler or a guarantee against blocked kernel I/O. SIGKILL cannot run user-space cleanup. Input, time and PDF-size limits are not independent compiler memory isolation, PHP-FPM concurrency sizing or denial-of-service capacity evidence.

Registration must succeed before generation begins. On a lost registration response, the wrapper replays the identical POST once with the same ID and fields within the remaining total budget. This lets the Server compare account, original filename and source hash; GET omits those registration fields and cannot prove that an old ready request matches this invocation. An authoritative replay conflict is rejected. On a lost upload response, the wrapper can query only the same source/request ID within the remaining budget. It returns zero only for a validated matching `ready` fact with document and admission; after an upload it additionally checks the PDF hash/length and the unchanged registration snapshots. A still-preparing result, failed verification, malformed response or exhausted budget yields `结果无法确认` and nonzero exit, never a new ID or a claim that nothing was accepted. There is no automatic later retry, local queue or outbox. Generation failure is reported best effort with only a fixed failure enum while time remains; a lost report may honestly leave a preparing request.

Without `--request-id`, every invocation generates a new canonical UUIDv7. Staff should retain the printed ID when an outcome is unclear and verify it through the dedicated source API before resubmitting. Only a controlled retry with the same explicit ID and exactly the same account, original filename and source hash is an idempotent registration retry. A fresh contestant submission can create another task; no cross-invocation exactly-once claim is made. These snapshots are not live queue ranking, ETA, paper inventory or evidence of physical completion.
