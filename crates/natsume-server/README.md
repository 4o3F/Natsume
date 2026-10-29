# Server

For the complete deployment procedure, see the Chinese [operations runbook](../../docs/operations-deployment.zh-CN.md), including CA creation, Server installation, building the Client Deb from source and operational checks.

For first installation, use the packaged [`natsume-server-setup`](../natsume-server-setup/README.md) interactively after installing the Deb. It generates or imports deployment materials, runs bootstrap as the service user and verifies startup. Later configuration and certificate changes remain manual.

The Server provides a TLS 1.3-only, HTTP/1.1-only listener with the unauthenticated `GET /api/v2/health` process-liveness route.

The single `natsume-server` binary has exactly three mandatory modes and no custom arguments or flags. All three load only `/etc/natsume-server/config.toml`; argv never carries configuration, paths, or secrets.

A reloadable stderr tracing subscriber is installed before CLI parsing or configuration loading. Configuration failures are logged as `ERROR server configuration failed config_path="/etc/natsume-server/config.toml" error=<Snafu report>`, with a specific reason: file read errors, missing required fields, TOML line/column, or a field's validation rule. Rejected values and source lines are not printed. For example, an HTTP DOMjudge origin reports that `runtime.domjudge_origin` requires HTTPS. Command failures use `ERROR server command failed config_path="/etc/natsume-server/config.toml" error=<Snafu report>`. There are no timestamps, ANSI escapes or target names. After configuration loads, the same subscriber applies `[log].level` and activates OTLP tracing when configured; stderr formatting stays unchanged.

- `natsume-server serve` opens an existing database, runs migrations and provisioning close-once recovery, requires an existing valid vault master key and initialized operator account, synchronizes the deployment's DOMjudge origin to Runtime Config, validates the TLS identity, and then binds. It never creates a database, key, or account and never prompts.
- `natsume-server bootstrap` creates or migrates the database, creates the vault master key only when absent, reads the login name and password from a TTY (password twice without echo), atomically creates the single first admin and initializes Runtime Config from the deployment's DOMjudge origin, and exits without TLS preflight or a listener. Repeating it makes zero business writes and exits non-zero.
- `natsume-server reset-operator-password` opens the existing database, runs migrations, and reads the target login name and new password from a TTY (password twice without echo). In one transaction it replaces that operator's PHC string, advances its credential revision, and purges all of that operator's current sessions. It never creates accounts or touches the vault master key. An unknown login name exits non-zero with zero writes.

## TLS and Origin CA material

The generic Server Deb includes `natsume-server`, `/usr/bin/natsume-server-setup`, Web assets, service and `/usr/share/doc/natsume-server/config.example.toml`. Deployment supplies the complete `/etc/natsume-server/config.toml`, `/etc/natsume/trust/control-ca.crt` and `/etc/natsume/trust/local-origin-ca.crt`, all `root:root`, mode `0644`, with parent directories mode `0755`. Package scripts never generate or rewrite these files; fresh install, reinstall, removal and purge preserve deployment inputs. The systemd service skips startup while any required file is missing.

The single configuration contains `[listen]`, `[log]`, `[storage]`, `[tls]`, `[site]`, `[trust]` and `[runtime]`; see the [deployment example](../../packaging/server/config.example.toml). `[site]` holds `gateway_hostname`, `gateway_not_after` and `contest_end`; the expiry must cover contest end plus one day. `[trust]` holds `control_root` and `local_origin_root` paths. The Gateway hostname and two CA certificates must match the Client deployment.

`[runtime].domjudge_origin` is required and must be a canonical HTTPS origin, such as `https://judge.contest.example`, without credentials, a path, trailing slash, query or fragment. `bootstrap` initializes its database row in the same transaction as the first administrator; either both are committed or neither is. Schema migrations and vault key creation happen before that transaction and may remain after a failed bootstrap. Contest and device tables are populated later by imports and enrollment. Changing the origin requires updating this configuration and restarting the service; no manual database writes are needed.

Before starting `natsume-server serve`, provision Origin CA issuing material alongside the Server TLS leaf/key pair through the deployer-controlled PKI workflow defined in the [target architecture](../../docs/architecture.md#53-pki).

The two Origin CA files have fixed names under the Server private keys directory:

- `/var/lib/natsume-server/keys/origin-ca.der` is one X.509 certificate encoded as DER.
- `/var/lib/natsume-server/keys/origin-ca-key.pk8` is its matching private key encoded as unencrypted PKCS#8 DER.

The private keys directory must be owned by `natsume-server:natsume-server` with mode `0700`; both files must have the same ownership and mode `0600`. `serve` validates both encodings, their public-key match and CA basicConstraints before binding. An Origin CA keyUsage extension, when present, must permit `keyCertSign`. The CA certificate must also be the exact certificate provisioned on the Server and injected into the Client image as `/etc/natsume/trust/local-origin-ca.crt` (PEM there): startup decodes that public certificate to DER and requires byte-for-byte equality with `origin-ca.der`. Missing, malformed, mismatched, or overly broad private material fails closed. `bootstrap`, reset, package install, and package upgrade never create or rewrite these files.

The first-install setup tool additionally requires Origin CA P-256, explicit CA keyUsage, validity coverage through `gateway_not_after`, and a directly trusted Server leaf chain covering every advertised IP SAN and suitable for serverAuth. These stricter policies are checked only by setup; `serve` does not perform them or a probe signature. Runtime hardening remains follow-up work.

The server embeds and runs its Diesel migrations at runtime; deployed packages and production hosts do not require Diesel CLI. Developers and CI use exactly `diesel_cli 2.3.12` only for `deno task schema:check`, which rebuilds the committed private `diesel/{business,journal}/schema.rs` artifacts from separate temporary databases and checks their clean diffs. CI installs it with `cargo install diesel_cli --version 2.3.12 --locked --no-default-features --features sqlite-bundled`.

For the manual first-install workflow, install the package and complete the fixed non-secret configuration, then open an interactive TTY and run:

```console
sudo -u natsume-server -- /usr/bin/natsume-server bootstrap
```

Enter the login name and the same password twice at the prompts, then start the `natsume-server.service`. Do not run bootstrap as root or from automation. The package `postinstall` must not run it because install-time secret handling and a packaging-script TTY prompt are forbidden. The service always invokes `natsume-server serve`.

For offline operator credential recovery, open an interactive TTY and run:

```console
sudo -u natsume-server -- /usr/bin/natsume-server reset-operator-password
```

Enter the target login name and the same new password twice. Reset advances the credential revision even for an unchanged password, fencing pending sign-ins verified against older credentials. Run only as the service user from a TTY, never as root, from automation or in package `postinstall`.

## HTTP request bodies

The Panel API does not set per-route body-size overrides. JSON and Bytes extraction use Axum's default 2 MiB limit. JSON extraction failures retain the API's 400 error mapping; oversized XLSX uploads return 413. Format-specific constraints remain owned by their components, independently of the HTTP extractor limit. See the [HTTP boundary rules](../../docs/architecture.md#153-desired-state-operator-api).

Import admits one heavy operation at a time before buffering the workbook, shared by preview creation and commit. Busy requests return 503 with `Retry-After: 1`, without reading or queueing their bodies. Lightweight import queries remain available. Capacity remains reserved until queued/running blocking work and its transaction finish, even if the caller is cancelled. Parsed cell text is separately bounded to 64 MiB, counting repeated shared-string references before copying.

The current `axum::serve` entry point does not install a Hyper timer, so the default HTTP/1 request-header timeout is not active. There is also no API-wide processing deadline or independent body-read timer; slow uploads can retain Import capacity until the request ends. See [resource-protection verification](docs/resource-limits.md) for tests, opt-in measurements and acceptance limitations.

## DOMjudge print admission

The Print component registers source-scoped requests and validates final PDFs. It syncs immutable `<sha256>.pdf` files into `[storage].print_documents` before committing their metadata, FIFO sequence and first admission snapshot in SQLite. No PDF bytes are written in the admission transaction. The `00000000000007_print` migration creates metadata-only document records, and startup verifies every referenced file before serving. The [Print notes](docs/print-admission.md) describe structural PDF validation, failure recovery and coordinated database/document backups; the [architecture contract](../../docs/architecture.md#2321-i1-受理协议与独立交付) defines fields, transitions and failure semantics.

Administrators create and list dedicated sources at `/api/v2/print/sources` and revoke one at `/api/v2/print/sources/{source_id}`. Creation returns a token only once; store it in the restricted file on the DOMjudge host, never in the package, command arguments or logs. The Web `/print` page shows the global admitted queue to administrators and viewers through `GET /api/v2/print/queue`. Administrators can requeue an exhausted document from its details after confirming the risk of duplicate printing; viewers remain read-only. The independently packaged [DOMjudge wrapper](../../packaging/print-domjudge/README.md) uses that source token over strictly verified HTTPS and maps `[username]` exactly to the existing roster account. It needs no shared directory or Server package on the DOMjudge host.

Only `PUT /api/v2/print/requests/{request_id}/document` has an 8 MiB body-limit override; other routes keep the default 2 MiB limit. There is no added Server body-read deadline. The wrapper separately limits UTF-8 source to 256 KiB, compilation to two seconds and total synchronous execution to five seconds. Documents are retained without automatic cleanup. The 8 MiB limit covers compressed upload bytes only; parsing has no independent memory isolation.

After `deno task prepare print-domjudge`, run the native Cargo commands in the [opt-in Print tests](../../packaging/print-domjudge/README.md#opt-in-tests), including the explicit real HTTPS/Typst integration test. The integration test uses private fixtures, the actual Server TLS listener/router and wrapper library, not host configuration or a deployed DOMjudge/PHP process. It checks same-ID replay, intentional distinct-ID submission, a simulated lost success response after commit, a hidden registration conflict against an old ready request, independent database reopen, trust rejection and revocation.

## Complete roster import

Preparation accepts the fixed [Teams XLSX template](src/component/import/examples/template.xlsx). The [import format guide](docs/roster.md#excel-模板) defines all nine columns, school merging, text-only cells and size limits. XLSX parsing and input types belong to the internal Import component. Every upload contains the complete roster; omitted accounts, teams, schools and seats appear as removals.

Administrators download the template with `GET /api/v2/imports/template`, send raw XLSX bytes to `POST /api/v2/imports`, and review the non-secret diff. Commit resends the workbook to `POST /api/v2/imports/{import_id}/actions/commit` with `x-natsume-preview-token`. Both uploads use `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`. HTTP uploads are limited to 2 MiB by Axum's Bytes extractor. Import separately enforces an internal 8 MiB workbook format limit. A pending preview expires after 30 minutes; the File and token stay in browser memory only. Reloading requires discard and a new upload.

Import owns `organizations` and `teams` as well as the existing seats, accounts, mappings and encrypted credentials. Schools receive `INST-001`-style IDs in school-name order on first import. Later imports reuse current school IDs and allocate new ones after the persisted SQLite sequence, including after deletion. Renaming a school's matching name creates a new school and ID in the preview. No school IDs are allocated by preview alone.

Commit rereads and verifies the baseline, non-secret candidate and password-change set in one SQLite transaction. Only changed passwords rewrite the vault and advance credential revisions. An identical roster causes no business writes. Metadata updates preserve account/seat IDs, device bindings and session targets; seat swaps preserve the binding on its seat. Removing an occupied seat is blocked. Failed commits roll back the whole roster and retain the candidate for recovery.

The roster migration adds the new tables and discards obsolete pending previews; existing devices, control keys, bindings, accounts, vault records and session/home targets are preserved. Existing installations supply missing team and school metadata through the full XLSX import. Never clear the database to apply this migration.

## School logos and DOMjudge export

Configure the deployment-owned source directory in the same `config.toml`:

```toml
[storage]
# Alongside database and root_key:
organization_logos = "/var/lib/natsume-server/organization-logos"
print_documents = "/var/lib/natsume-server/print-documents"
```

This absolute path is required; a missing directory is allowed and means missing logos. The Server Deb creates the default directory shown above during installation and reinstallation, owned by `root:natsume-server` with mode `0750`, preserving existing images. Copy images as `root:natsume-server` with mode `0640`. A custom directory is deployment-owned and must grant the service user read/search access. Name each source file after the complete Chinese or English school name. There is no per-row filename or alias. Only direct children with PNG/JPEG/WebP/SVG extensions participate; actual content chooses the decoder. Symlinks are rejected. Source files stay unchanged. Matching, decoding and SVG rendering belong to the internal Contest component; Preparation Center exposes the school-logo checks.

Preparation Center shows every committed or candidate school with its INST ID, source filenames, thumbnail and available/missing/ambiguous/invalid status. Search, problem filtering and manual refresh work without a restart or re-import. Candidate IDs apply only after commit. Administrators use these API routes:

- `GET /api/v2/organizations`
- `GET /api/v2/imports/{import_id}/organizations`
- `GET /api/v2/imports/{import_id}/organizations/{organization_id}/logo`
- `GET /api/v2/exports/domjudge`

`GET /api/v2/organizations/{organization_id}/logo` is public, read-only, and only serves a school present in the committed roster. It never lists a directory or returns team credentials. Raster images retain their original bytes and actual MIME type; SVG is rendered to PNG. SHA-256 ETags support conditional requests, including weak `If-None-Match` validators. Successful images require revalidation; 404/errors and candidate images use `no-store`. Later requests see file replacements.

**Download DOMjudge ZIP** contains `groups.json`, `organizations.json`, `teams.json`, `accounts.yaml` and every available school logo as `logos/INST-xxx.png`. All data comes from one committed SQLite read transaction, including encrypted vault records. The transaction closes before decryption and image conversion. Contest owns this read/export boundary; Import remains the only roster writer. Only Administrators can download the ZIP, which contains current plaintext passwords, uses `no-store`, and is not saved on the Server or in browser storage. The browser hands the response to its normal download manager.

The export uses the modern DOMjudge JSON/YAML contract, stable school IDs and fixed account IDs. Names are `中文名(English name)` when both exist. It supplies no invented `icpc_id`. YAML serialization preserves strings such as leading-zero passwords. Available PNG/JPEG/WebP/SVG files become 64×64 PNGs. SVG is first rendered at its natural 96 DPI canvas; all logos are then resampled with Lanczos3 and premultiplied alpha, preserving aspect ratio and centered on a transparent canvas without stretching or cropping. Source files and HTTP logo delivery stay unchanged. Missing or ambiguous images are omitted without blocking import/export. A matched unreadable, corrupt or oversized file aborts the download with a school/file diagnostic.

Work is bounded to four image workers and one export worker; the in-memory ZIP is limited to 256 MiB. Source files have an 8 MiB limit and decoded images are limited to 4096 pixels per side / 4 Mi pixels total. Image IO and encoding run off the async control loop. File changes are independent of the database read snapshot. Back up both the database/vault and source logo directory.

Bound control targets include required bilingual team/school presentation from the same binding transaction. Missing profiles fail closed; import the full roster before reconnecting migrated bound devices. Client waiting downloads logos from this Server with the pinned Control CA, retains display-only data offline, and clears it on unbind. The matching Server and Client use `natsume.control.v3`; the Daemon and Session Agent use local protocol 3. See the packaged [Session Agent runbook](../../packaging/client/rootfs/usr/share/doc/natsume-client/session-agent-gui-startup.md) for cache ownership, display and recovery behavior.

## OpenTelemetry traces

The Server always writes its ordinary `tracing` output to stderr. OTLP/gRPC trace export is enabled only when `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is non-empty; `OTEL_SDK_DISABLED=true` disables it. Other exporter settings use the standard OpenTelemetry environment variables. The process explicitly shuts down the trace provider on normal exit. Exporter construction failures fail startup, while shutdown/export failures only write a fixed diagnostic and never replace the business command result.

Traces provide best-effort diagnostics; business audit storage and a Panel audit page are outside scope. HTTP spans extract W3C trace context, and Diesel emits redacted query spans that do not contain SQL text, bind values, database URLs, or error details.
