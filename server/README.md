# Server

Stage 3 provides a TLS 1.3-only, HTTP/1.1-only listener with the unauthenticated
`GET /api/v2/health` process-liveness route.

The single `natsume-server` binary has exactly two mandatory modes and no custom
arguments or flags. Both load only `/etc/natsume-server/config.toml`; argv never
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

Known Gate limitations: the 4096-byte session body limit is closed. Header
count/size and slow-header handling remain open while Stage 4 retains
`axum::serve`; connection capacity remains `ENV-UNFROZEN` pending S0-4 Device
fleet evidence for the shared future WSS port. Gate 4 is therefore not fully
`PASS`. `INV-CERT` integration remains pending, and target platform, browser,
network, and PKI inputs remain `ENV-UNFROZEN`.
