# Server

Stage 3 provides a TLS 1.3-only, HTTP/1.1-only listener with the unauthenticated
`GET /api/v2/health` process-liveness route.

The single `natsume-server` binary has exactly three mandatory modes and no custom
arguments or flags. All three load only `/etc/natsume-server/config.toml`; argv never
carries configuration, paths, or secrets.

- `natsume-server serve` opens an existing database, runs migrations and
  provisioning close-once recovery, requires an existing valid vault master
  key, validates the TLS identity, and then binds. It never creates a database,
  key, or account and never prompts.
- `natsume-server bootstrap` creates or migrates the database, creates the vault
  master key only when absent, reads the login name and password from a TTY
  (password twice without echo), atomically creates the single first admin,
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
`[site]` and `[trust]`; see the [deployment example](../packaging/server/config.example.toml).
`[site]` holds `gateway_hostname`, `gateway_not_after` and `contest_end`; the
expiry must cover contest end plus one day. `[trust]` holds `control_root` and
`local_origin_root` paths. The Gateway hostname and two CA certificates must
match the Client deployment. There is no second site configuration file.

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
