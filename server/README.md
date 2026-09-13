# Server

For the complete deployment procedure, see the Chinese
[operations runbook](../docs/operations-deployment.zh-CN.md), including CA creation,
Server installation, building the Client Deb from source and operational checks.

Stage 3 provides a TLS 1.3-only, HTTP/1.1-only listener with the unauthenticated
`GET /api/v2/health` process-liveness route.

The single `natsume-server` binary has exactly three mandatory modes and no custom
arguments or flags. All three load only `/etc/natsume-server/config.toml`; argv never
carries configuration, paths, or secrets.

Configuration failures are printed to stderr before logging starts, with the
configuration path and a specific reason: file read errors, missing required
fields, TOML line/column, or a field's validation rule. Rejected values and source
lines are not printed. For example, an HTTP DOMjudge origin reports that
`runtime.domjudge_origin` requires HTTPS.

- `natsume-server serve` opens an existing database, runs migrations and
  provisioning close-once recovery, requires an existing valid vault master
  key and initialized operator account, synchronizes the deployment's DOMjudge
  origin to Runtime Config, validates the TLS identity, and then binds. It never
  creates a database, key, or account and never prompts.
- `natsume-server bootstrap` creates or migrates the database, creates the vault
  master key only when absent, reads the login name and password from a TTY
  (password twice without echo), atomically creates the single first admin and
  initializes Runtime Config from the deployment's DOMjudge origin,
  and exits without TLS preflight or a listener. Repeating
  it makes zero business writes and exits non-zero.
- `natsume-server reset-operator-password` opens the existing database, runs
  migrations, and reads the target login name and new password from a TTY
  (password twice without echo). In one transaction it replaces that operator's
  PHC string, advances its credential revision, and purges all of that operator's
  current sessions. It never creates accounts or touches the vault master key.
  An unknown login name exits non-zero with zero writes.

## TLS and Origin CA material

The generic Server Deb includes the binary, Web assets, service and
`/usr/share/doc/natsume-server/config.example.toml`. Deployment supplies the
complete `/etc/natsume-server/config.toml`, `/etc/natsume/trust/control-ca.crt`
and `/etc/natsume/trust/local-origin-ca.crt`, all `root:root`, mode `0644`, with
parent directories mode `0755`. Package scripts never generate or rewrite these
files; fresh install, reinstall, removal and purge preserve deployment inputs.
The systemd service skips startup while any required file is missing.

The single configuration contains `[listen]`, `[log]`, `[storage]`, `[tls]`,
`[site]`, `[trust]` and `[runtime]`; see the
[deployment example](../packaging/server/config.example.toml).
`[site]` holds `gateway_hostname`, `gateway_not_after` and `contest_end`; the
expiry must cover contest end plus one day. `[trust]` holds `control_root` and
`local_origin_root` paths. The Gateway hostname and two CA certificates must
match the Client deployment. There is no second site configuration file.

`[runtime].domjudge_origin` is required and must be a canonical HTTPS origin,
such as `https://judge.contest.example`, without credentials, a path, trailing
slash, query or fragment. `bootstrap` initializes its database row in the same
transaction as the first administrator; either both are committed or neither is.
Schema migrations and vault key creation happen before that transaction and may
remain after a failed bootstrap. Contest and device tables are populated later by
imports and enrollment. Changing the origin requires updating this configuration
and restarting the service; no manual database writes are needed.

Before `natsume-server serve` starts, the deployer must provision the Origin CA
issuing material exactly as it provisions the Server TLS leaf/key pair. Packaging
must never generate either CA. The
[target architecture](../docs/architecture.md#53-pki) keeps CA creation and
custody in the deployer-controlled PKI workflow.

The two Origin CA files have fixed names under the Server private keys directory:

- `/var/lib/natsume-server/keys/origin-ca.der` is one X.509 certificate encoded
  as DER.
- `/var/lib/natsume-server/keys/origin-ca-key.pk8` is its matching private key
  encoded as unencrypted PKCS#8 DER.

The private keys directory must be owned by `natsume-server:natsume-server` with
mode `0700`; both files must have the same ownership and mode `0600`. `serve`
validates both encodings, their public-key match, and a probe signature before
binding. The CA certificate must also be the exact certificate provisioned on
the Server and injected into the Client image as
`/etc/natsume/trust/local-origin-ca.crt` (PEM there):
startup decodes that public certificate to DER and requires byte-for-byte
equality with `origin-ca.der`. Missing, malformed, mismatched, or overly broad
private material fails closed. `bootstrap`, reset, package install, and package
upgrade never create or rewrite these files.

The server embeds and runs its Diesel migrations at runtime; deployed packages
and production hosts do not require Diesel CLI. Developers and CI use exactly
`diesel_cli 2.3.12` only for `just diesel-schema`, which rebuilds the committed
private `diesel/schema.rs` artifact from a temporary database and checks the clean diff.
CI installs it with `cargo install diesel_cli --version 2.3.12 --locked
--no-default-features --features sqlite-bundled`.

For initial provisioning, install the package and complete the fixed non-secret
configuration, then open an interactive TTY and run:

```console
sudo -u natsume-server -- /usr/bin/natsume-server bootstrap
```

Enter the login name and the same password twice at the prompts, then start the
`natsume-server.service`. Do not run bootstrap as root or from automation. The
package `postinstall` must not run it because install-time secret handling and a
packaging-script TTY prompt are forbidden. The service always invokes
`natsume-server serve`.

For offline operator credential recovery, open an interactive TTY and run:

```console
sudo -u natsume-server -- /usr/bin/natsume-server reset-operator-password
```

Enter the target login name and the same new password twice at the prompts. The
command atomically replaces that operator's PHC string, advances its credential
revision, purges all of that operator's current sessions, and exits. Pending
sign-ins can create a session only while their verified credential revision is
still current; even resetting to the same password fences older sign-ins. It
never creates an account and never touches the vault master key; an unknown
login name exits non-zero with zero writes. Never run it as root or from
automation. The package `postinstall` must not call it because install-time
secret handling and a packaging-script TTY prompt are forbidden.

## Complete roster import

Preparation accepts the fixed [Teams XLSX template](../crates/roster/examples/template.xlsx).
The [roster library](../crates/roster/README.md) defines all nine columns, school
merging, text-only cells and size limits. Every upload contains the complete
roster; omitted accounts, teams, schools and seats appear as removals. The old
CSV and JSON commit bodies are rejected.

Administrators download the template with `GET /api/v2/imports/template`, send
raw XLSX bytes to `POST /api/v2/imports`, and review the non-secret diff. Commit
resends the workbook to `POST /api/v2/imports/{import_id}/actions/commit` with
`x-natsume-preview-token`. Both uploads use
`application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` and have an
8 MiB body limit. A pending preview expires after 30 minutes; the File and token
stay in browser memory only. Reloading requires discard and a new upload.

Import owns `organizations` and `teams` as well as the existing seats, accounts,
mappings and encrypted credentials. Schools receive `INST-001`-style IDs in
school-name order on first import. Later imports reuse current school IDs and
allocate new ones after the persisted SQLite sequence, including after deletion.
Renaming a school's matching name creates a new school and ID in the preview.
No school IDs are allocated by preview alone.

Commit rereads and verifies the baseline, non-secret candidate and password-change
set in one SQLite transaction. Only changed passwords rewrite the vault and
advance credential revisions. An identical roster causes no business writes.
Metadata updates preserve account/seat IDs, device bindings and session targets;
seat swaps preserve the binding on its seat. Removing an occupied seat is blocked.
Failed commits roll back the whole roster and retain the candidate for recovery.

The roster migration adds the new tables and discards obsolete pending previews;
existing devices, control keys, bindings, accounts, vault records and session/home
targets are preserved. Existing installations supply missing team and school
metadata through the full XLSX import. Never clear the database to apply this migration.

## School logos and DOMjudge export

Configure the deployment-owned source directory in the same `config.toml`:

```toml
[storage]
# Alongside database and root_key:
organization_logos = "/var/lib/natsume-server/organization-logos"
```

This absolute path is required; a missing directory is allowed and means missing
logos. Give the service user read/search access. Name each source file after the
complete Chinese or English school name. There is no per-row filename or alias.
Only direct children with PNG/JPEG/WebP/SVG extensions participate; actual content
chooses the decoder. Symlinks are rejected. Source files stay unchanged.
`natsume-check-logos` uses the same matching, decoding and SVG rendering rules.

Preparation Center shows every committed or candidate school with its INST ID,
source filenames, thumbnail and available/missing/ambiguous/invalid status. Search,
problem filtering and manual refresh work without a restart or re-import. Candidate
IDs apply only after commit. Administrators use these API routes:

- `GET /api/v2/organizations`
- `GET /api/v2/imports/{import_id}/organizations`
- `GET /api/v2/imports/{import_id}/organizations/{organization_id}/logo`
- `GET /api/v2/exports/domjudge`

`GET /api/v2/organizations/{organization_id}/logo` is public, read-only, and only
serves a school present in the committed roster. It never lists a directory or
returns team credentials. Raster images retain their original bytes and actual
MIME type; SVG is rendered to PNG. SHA-256 ETags support conditional requests,
including weak `If-None-Match` validators. Successful images require revalidation;
404/errors and candidate images use `no-store`. Later requests see file replacements.

**Download DOMjudge ZIP** contains `groups.json`, `organizations.json`, `teams.json`,
`accounts.yaml`, `README.md` and every available school logo as `logos/INST-xxx.png`.
All data comes from one committed SQLite read transaction, including encrypted
vault records. The transaction closes before decryption and image conversion.
Contest owns this read/export boundary; Import remains the only roster writer.
Only Administrators can download the ZIP, which contains current plaintext
passwords, uses `no-store`, and is not saved on the Server or in browser storage.
The browser hands the response to its normal download manager.

The export uses the modern DOMjudge JSON/YAML contract, stable school IDs and fixed
account IDs. Names are `中文名(English name)` when both exist. It supplies no invented
`icpc_id`. YAML serialization preserves strings such as leading-zero passwords.
Available PNG/JPEG/WebP/SVG files become real PNG, preserving raster dimensions and
alpha; SVG uses its natural 96 DPI canvas. Missing/ambiguous images are documented
in the ZIP README without blocking import/export. A matched unreadable, corrupt or
oversized file aborts the download with a school/file diagnostic. The README also
contains import order, CLI commands and affiliation image installation instructions.

Work is bounded to four image workers and one export worker; the in-memory ZIP is
limited to 256 MiB. Source files have an 8 MiB limit and decoded images are limited
to 4096 pixels per side / 4 Mi pixels total. Image IO and encoding run off the async
control loop. File changes are independent of the database read snapshot. Back up
both the database/vault and source logo directory.

TODO(roster-presentation): deliver protocol and Client waiting display/cache in the
next stage. This export does not push changes into DOMjudge; import the ZIP's files
there and verify team logins after password rotation.

## OpenTelemetry traces

The Server always writes its ordinary `tracing` output to stderr. OTLP/gRPC
trace export is enabled only when `OTEL_EXPORTER_OTLP_ENDPOINT` or
`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is non-empty; `OTEL_SDK_DISABLED=true`
disables it. Other exporter settings use the standard OpenTelemetry environment
variables. The process explicitly shuts down the trace provider on normal exit.
Exporter construction failures fail startup, while shutdown/export failures only
write a fixed diagnostic and never replace the business command result.

Traces are best-effort observability data, not a business audit trail. The
Server does not write a local operation JSONL file or expose a Panel audit page.
HTTP spans extract W3C trace context, and Diesel emits redacted query spans that
do not contain SQL text, bind values, database URLs, or error details.

The complete Server target, including the component model and Device WSS, is
defined only by the [target architecture](../docs/architecture.md).
