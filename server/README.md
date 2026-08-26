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
  (password twice without echo), atomically creates the single first admin with
  its typed audit row, and exits without TLS preflight or a listener. Repeating
  it makes zero business writes and exits non-zero.
- `natsume-server reset-operator-password` opens the existing database, runs
  migrations, and reads the target login name and new password from a TTY
  (password twice without echo). In one transaction it replaces that operator's
  PHC string, purges all of that operator's current sessions, and writes one
  `system:password-reset` audit row. It never creates accounts or touches the
  vault master key. An unknown login name exits non-zero with zero writes.

## TLS and Origin CA material

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
binding. The CA certificate must also be the exact certificate supplied to the
package as `/etc/natsume/trust/local-origin-ca.crt` for Client trust (PEM there):
startup decodes that packaged certificate to DER and requires byte-for-byte
equality with `origin-ca.der`. Missing, malformed, mismatched, or overly broad
private material fails closed. `bootstrap`, reset, package install, and package
upgrade never create or rewrite these files.

The server embeds and runs its Diesel migrations at runtime; deployed packages
and production hosts do not require Diesel CLI. Developers and CI use exactly
`diesel_cli 2.3.12` only for `just diesel-schema`, which rebuilds the committed
private schema artifact from a temporary database and checks the clean diff.
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
command replaces that operator's PHC string, purges all of that operator's
current sessions, writes one `system:password-reset` audit row, and exits. It
never creates an account and never touches the vault master key; an unknown
login name exits non-zero with zero writes. Never run it as root or from
automation. The package `postinstall` must not call it because install-time
secret handling and a packaging-script TTY prompt are forbidden.

The complete Server target, including the component model and Device WSS, is
defined only by the [target architecture](../docs/architecture.md).
