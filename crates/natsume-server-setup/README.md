# Server setup

`natsume-server-setup` is the interactive first-install tool shipped in the Server Deb. It collects deployment parameters, generates or imports PKI materials, publishes deployment inputs, runs bootstrap as the service user, starts systemd and verifies HTTPS health. See the Chinese [operations runbook](../../docs/operations-deployment.zh-CN.md) for installation, offline signing and Client handoff; [architecture §5.3](../../docs/architecture.md#53-pki) defines the trust boundary.

It supports first installation only. It does not reconfigure, renew certificates, replace CAs, repair or upgrade an installed system. If any of these seven paths exists, including an occupied symlink, setup refuses to proceed:

- `/etc/natsume-server/config.toml`
- `/etc/natsume/trust/control-ca.crt`
- `/etc/natsume/trust/local-origin-ca.crt`
- `/var/lib/natsume-server/keys/origin-ca.der`
- `/var/lib/natsume-server/keys/origin-ca-key.pk8`
- `/var/lib/natsume-server/keys/server-tls-leaf.der`
- `/var/lib/natsume-server/keys/server-tls-key.pk8`

The business database, journal database, their `-wal`/`-shm` files and `server-root.key` are excluded from this detection so a rolled-back first installation can resume safely. Existing deployments follow the runbook's [manual maintenance procedure](../../docs/operations-deployment.zh-CN.md#84-配置证书和版本更新).

## Run and prerequisites

Install the matching Server Deb first, then use a root terminal on Linux amd64 with systemd running:

```console
sudo apt install ./natsume-server_<version>_amd64.deb
sudo natsume-server-setup
```

Stdin, stdout and stderr must all be TTYs; bootstrap also needs the inherited controlling terminal. Do not redirect stderr: prompts and answer context are rendered there. Read diagnostics on the terminal without recording sessions containing key exports. The package must have supplied `/usr/bin/natsume-server`, the `natsume-server` account and state directory. Setup reports unsynchronized or unknown clock status and requires confirmation to continue. Check time, networking and the external HTTPS DOMjudge beforehand; setup does not configure these services or the firewall. Package maintainer scripts never invoke setup or bootstrap.

`--pki-dir` selects an absolute administrator PKI directory, default `/root/natsume-pki`; for example, use `sudo natsume-server-setup --pki-dir /root/contest-pki` instead, consistently across CSR runs. It is root-owned, mode `0700`, separate from service keys. Existing directory metadata must match the required ownership and exact modes; existing files are not overwritten. There is no answer-file or noninteractive mode.

Prompts are in English. Enter one to sixteen distinct Server IPs separated by commas, HTTPS port `1024–65535` (default `8443`), Gateway hostname, canonical DOMjudge HTTPS origin, contest end and Gateway expiry. The first IP is used for Client configuration and health checks. All IPs must appear in the leaf's IP SANs. IPv4-only input listens on `0.0.0.0`; any IPv6 input selects `[::]`. Verify mixed-family reachability in a VM.

Before starting, check the Server VM's timezone with `timedatectl status`. Preflight displays the detected timezone; an unknown timezone requires warning acknowledgement. Select each deadline's date in the calendar panel, then enter its time as strict two-digit `HH:MM` (`00:00–23:59`, 24-hour clock). Dates and times are interpreted in the **Server host's local timezone** and converted to UTC. Contest end date defaults to local today and cannot be earlier; its time has no default. Gateway expiry date defaults to the next calendar day and cannot precede the contest date; its time defaults to the contest time. Expiry must still be future and at least `contest_end + 86400s`, including across timezone transitions. Nonexistent or ambiguous local times show an explanation and require another time. Esc from a time prompt returns to its date; Esc from a date returns to the preceding parameter.

Each completed deadline and the confirmation plan show local time, timezone name, effective UTC offset and the resulting UTC timestamp. Configuration, `DeploymentParameters` and pending `request.toml` retain strict RFC 3339 UTC `Z` strings with seconds `00` for new calendar input. CSR continuation reuses the saved UTC parameters without asking for dates or times again.

## Certificate sources and validation

| Control CA source | Issue a Server leaf | Import leaf + key | Export CSR | Default TLS source |
| --- | --- | --- | --- | --- |
| Generate a new CA (default) | Available | Unavailable | Unavailable | Issue |
| Import certificate + private key | Available | Available | Available | Issue |
| Import certificate only; key stays offline | Unavailable | Available | Available | CSR |

Local Origin CA independently defaults to generation or accepts an imported CA certificate and matching private key. It always requires ECDSA P-256, CA basicConstraints and explicit `keyCertSign`; its private key is needed online. Control CA may be a non-self-signed organizational CA used as the direct trust anchor; browser chain distribution remains the deployer's responsibility. Server sends one leaf, so it must be issued directly by the selected Control CA, without an intervening CA.

New CAs use P-256, path length zero and 3650-day validity, starting one hour before generation. A generated Server leaf starts one hour before generation and expires at the earlier of 365 days from generation or Control CA expiry; issuance is rejected if that cannot cover `gateway_not_after`.

Setup validates all materials before publishing deployment files: encoding, key pairing, CA constraints and usages, Origin P-256, current validity and coverage through Gateway expiry, direct Server chain, every advertised IP SAN and serverAuth suitability. Leaf expiry must not exceed Control CA expiry. Rendered configuration passes `ServerConfig::load_from` before publication.

This is stricter than `serve` (decision D8). Runtime validates TLS key loading/pairing, private-file permissions, Origin CA encoding, CA basicConstraints, key pairing and exact DER equality with the public Local Origin trust certificate; it rejects an Origin keyUsage extension that lacks `keyCertSign`, but accepts its absence. It does not enforce setup's P-256, explicit keyUsage, validity-coverage or Server leaf chain/SAN policy. Runtime does not perform a probe signature before binding. Later manual changes require the same deployment checks; runtime hardening is deferred.

## Imported key formats and one-time export

Each certificate is a single X.509 PEM or DER certificate; bundles and trailing material are rejected. Each input is limited to 1 MiB. Supported private keys are plaintext PKCS#8 PEM/DER, PBES2-encrypted PKCS#8 PEM/DER and plaintext named-curve SEC1 EC PEM/DER. SEC1 must include its public key for the ring provider. RSA PKCS#1 and legacy `Proc-Type` encrypted PEM are unsupported. Convert legacy PEM before importing:

```sh
openssl pkcs8 -topk8 -v2 aes-256-cbc -v2prf hmacWithSHA256 \
  -in old-key.pem -out encrypted-key.pk8.pem
```

PBES2 KDF costs are checked before decryption: PBKDF2 iterations `1–2,000,000`; scrypt N a power of two in `2–131,072`, r `1–8`, p `1–4` (about 128 MiB maximum working memory). Other KDFs are rejected. A wrong import passphrase can be retried three times before failure. Imported key originals are read into memory and are not archived separately; required online copies are still deployed below.

After reviewing and confirming the plan, each newly generated CA private key is displayed exactly once as encrypted PKCS#8 PEM, using its own nonempty, twice-confirmed export passphrase. Use a strong passphrase and save the encrypted export securely. The terminal then asks whether to save that encrypted key in the administrator PKI directory; default is **no**. Declining requires the literal `key saved` to acknowledge a separate secure copy, or `abort` to cancel. Saving creates `control-ca-key.pem` or `origin-ca-key.pem`, root-owned `0600`. A generated Control CA also creates public `control-ca.crt` there (`0644`). Control CA private keys never enter the service directory. Without a retained Control key, future certificates require the external key or offline signing.

The wizard attempts to clear the terminal scrollback after each export. Prompts, answer context and encrypted private-key export go directly to the terminal; secrets never enter tracing. Progress, results and errors use tracing. Do not record the terminal session containing an export.

## Published files

| Paths | Owner | Mode / encoding |
| --- | --- | --- |
| `/etc/natsume-server/config.toml` | `root:root` | `0644`, complete TOML |
| `/etc/natsume/trust/{control-ca,local-origin-ca}.crt` | `root:root` | `0644`, PEM |
| Configuration/trust parent directories | `root:root` | `0755` |
| `/var/lib/natsume-server/keys/` | `natsume-server:natsume-server` | `0700` |
| `keys/origin-ca.der`, `keys/server-tls-leaf.der` | `natsume-server:natsume-server` | `0600`, DER certificates |
| `keys/origin-ca-key.pk8`, `keys/server-tls-key.pk8` | `natsume-server:natsume-server` | `0600`, unencrypted PKCS#8 DER |
| Administrator PKI directory | `root:root` | `0700` |

The generated configuration sets `[storage].journal` to `/var/lib/natsume-server/natsume-journal.db` and writes the Server's default `[journal]` budgets: `max_age_days = 30`, `max_rows = 2000000`, `max_bytes = 536870912`, `min_free_bytes = 1073741824` and `flush_timeout_seconds = 10`, matching `packaging/server/config.example.toml`. No journal settings are prompted for. Server creates the journal as the service user on first startup; setup never creates, modifies or deletes either database. Rollback preserves the journal and both databases' `-wal`/`-shm` files. Adjust budgets after installation following [runbook §8.5](../../docs/operations-deployment.zh-CN.md#85-operational-event-journal日志库).

Origin CA DER is identical to the decoded public `local-origin-ca.crt`. Publication orders keys, trust, then configuration. Each file uses a same-directory temporary file, metadata setting, fsync and non-overwriting atomic publication, rejecting symlinks in deployment paths. Existing managed directories are checked through no-follow handles before confirmation and again during publication. `/etc` must be root-owned without group/other write access; `/var/lib/natsume-server` must match the packaged service ownership and `0750`. This is single-file atomicity, not a power-loss transaction across the entire set.

## Offline CSR and continuation

With CSR selected, the first run confirms the request plan and writes only `<pki-dir>/pending/`, root-owned `0700`, with four root-owned `0600` files:

- `server-tls-key.pk8`: unencrypted PKCS#8 DER; keep on the Server.
- `server-tls.csr`: PEM PKCS#10 request, including IP SANs and serverAuth request.
- `control-ca.crt`: expected issuer certificate.
- `request.toml`: parameters, format version, CA certificate/SPKI hashes and CSR public-key hash; no private key or passphrase.

It does not collect Origin CA materials or write system deployment inputs in this phase, and returns `PendingSignature` with exit code **0**. Ctrl-C during publication or the signing instructions removes only the new pending request and exits as aborted; cancellation while resuming preserves the existing pending request. Send only the public CSR, issuer certificate and request parameters to the offline CA. Sign directly with that Control CA, preserving all IP SANs, digitalSignature and serverAuth, covering Gateway expiry without exceeding CA validity. The runbook provides [offline OpenSSL commands](../../docs/operations-deployment.zh-CN.md#a10-离线-ca-签发-setup-生成的-csr).

Rerun the same command with the same `--pki-dir`. Setup revalidates pending ownership, permissions, parameters and hashes, asks for the signed leaf path, verifies issuer/signature/key/IPs, then collects Origin CA materials and presents the complete plan. Full certificate policy is checked before deployment. It does not ask for deployment parameters or Control CA again. Successful installation removes the four unchanged pending files and removes the directory if empty; unrelated entries are preserved. Failure before health success keeps the pending request for retry. To change pending parameters, deliberately discard the protected pending directory and start a new request; do not edit its record or replace its key.

## Bootstrap, systemd and Client handoff

After publication, setup drops to the service user's UID/GID for hidden deployment and bootstrap probes. The bootstrap probe opens the database read-only without migrations or WAL reconfiguration. An initialized database skips bootstrap; a missing or uninitialized database triggers `/usr/bin/natsume-server bootstrap` as `natsume-server`, inheriting the TTY. The Server collects administrator credentials directly, states its password rules and asks again after rejected input; setup does not receive them. A failed bootstrap is probed again before offering a retry, at most three attempts. Migrations and `server-root.key` can remain after a failed bootstrap.

Setup then runs systemd daemon-reload, enable and start. Health checks use the first IP, the selected Control CA as the sole trust anchor, TLS 1.3 and HTTP/1.1, without proxies or redirects. Each request has a five-second timeout and 4 KiB response limit; up to twenty attempts are separated by 1.5 seconds. Success requires HTTP 200 and JSON containing only `status: "ok"`. This checks process liveness, not DOMjudge, Client connectivity or physical printing.

After health succeeds, `<pki-dir>/handoff/<YYYYMMDDTHHMMSSZ>/` contains Client `config.toml`, `control-ca.crt`, `local-origin-ca.crt` and `SHA256SUMS`, all root-owned `0644` in `0755` handoff directories beneath the protected PKI directory. Copy only these public files to Client deployment staging and compare CA fingerprints over a trusted channel. The summary reports endpoints, CA fingerprints/validity, key retention and the handoff path. DOMjudge upstream certificate issuance stays manual.

## Failure, cancellation and exit status

The installation commits when HTTPS health passes. Before that point, Ctrl-C is handled during prompts, child processes and health checks; setup waits for interrupted child processes to exit, then rolls back. Earlier ordinary failures attempt to stop the service, disable it if this run enabled it, and remove only this attempt's unchanged deployment files, saved CA exports and newly created empty directories. Changed files and nonempty directories are preserved and cleanup failures are reported. Setup never deletes the business database, journal database, either database's `-wal`/`-shm` files or `server-root.key`. The Deb-supplied state directory is preserved even if recreated after planning. A rolled-back retry can therefore skip completed bootstrap and retain history from a previous service start. Discard every CA private key revealed during an aborted attempt and generate fresh keys; a terminal export cannot be recalled.

Before publishing deployment files, setup creates or verifies `<target-root>/var/lib/natsume-server-setup-rollback/` (root:root `0700`), retains its directory handle, and checks that it shares the keys directory's filesystem. Planning checks existing directory metadata and mount compatibility before confirmation. A preparation failure rejects installation before deployment writes. Rollback uses actual parent ownership and permissions: root-owned directories without group/other write permission (including `/etc` and administrator PKI storage) use fd device/inode/SHA-256 validation followed by same-directory `unlinkat`, without allocating quarantine entries. Service-writable directories use quarantine isolation before validation and deletion. Thus `/etc` may be on another filesystem; keys and quarantine must share one. Modified isolated objects are restored without overwriting the original path; restoration conflicts retain the object in quarantine and report both paths. An empty quarantine created by this attempt is removed after rollback (or when its report is dropped).

After health success, handoff export, summary display or pending cleanup failures are logged without uninstalling the working deployment; exit remains 0. Follow the reported fallback to create the public Client handoff manually. Do not delete deployed inputs to force setup to run again. Interrupted power or process termination may leave partial inputs; inspect and recover manually before retrying.

| Exit | Meaning |
| --- | --- |
| `0` | Installed, or CSR exported and awaiting signature (`PendingSignature`) |
| `1` | Prerequisite, material, filesystem, probe, bootstrap, service or health failure |
| `2` | Explicit abort / Ctrl-C cancellation, or invalid CLI arguments |

Esc navigates back before publication/key revelation where supported; at the initial step it cancels. Inspect rollback diagnostics even after cancellation; exit 2 does not establish that all cleanup succeeded.

The hidden `probe bootstrap` and `probe deployment` subcommands are internal, read-only checks requiring the `natsume-server` effective UID (root is rejected). They load the fixed Server configuration. Their protocol is `0` initialized/deployment passed, `10` database needs bootstrap, `11` database missing, `1` check/identity failure, `2` argument error. They do not offer a public status or repair command.

## Security limitations

Setup-owned secret buffers (PKCS#8 input/export material, planned key contents and rollback read buffers), setup temporary rcgen KeyPair serialization buffers, and the Server deployment facade's temporary P-256-check KeyPair serialization buffer are erased on drop using zeroization guards, including error returns. Setup issuers borrow guarded keys. rcgen's `Zeroize` implementation erases serialized DER only; cryptographic backend internal key copies and parser/transport library intermediates remain outside this tool's erasure control. `GatewayComponent::validate_der` creates a temporary runtime issuer whose KeyPair is exposed only by an immutable rcgen accessor: its owned serialized DER is not explicitly erased by setup. The serve runtime's long-lived issuer is unchanged and outside this phase's scope. inquire's intermediate password input/confirmation copies also remain outside this tool's erasure control. Terminal scrollback clearing depends on terminal support.

## Tests and acceptance

```sh
deno task prepare server
cargo test -p natsume-server-setup
cargo test -p natsume-server
```

Install OpenSSL on the test host: PKI tests invoke it for bidirectional interoperability, while production generation/import uses Rust libraries. Tests use temporary roots and scripted interactions/host adapters to cover source combinations, policy rejection, pending continuation, publication/rollback, bootstrap retries and HTTPS response limits. Non-root tests and Deb inspection do not establish real installation acceptance. Ubuntu 24.04 amd64 VM acceptance must exercise root paths, controlling TTY, service UID/GID, SQLite WAL behavior, actual systemd lifecycle and IPv4/IPv6 reachability.
