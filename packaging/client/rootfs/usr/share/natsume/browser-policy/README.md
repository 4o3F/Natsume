# Browser policy

At startup, the Device Daemon parses `[site].gateway_hostname` from
`/etc/natsume/config.toml` and passes the hostname to the Privileged Helper.
The Helper validates the hostname and updates these fixed files; it does not
read or parse the Client deployment config:

- `/etc/hosts`: a managed `BEGIN NATSUME GATEWAY` / `END NATSUME GATEWAY` block maps
  the hostname to `127.0.0.1` and `::1`; conflicting aliases for that hostname are
  removed while unrelated entries are preserved.
- `/etc/firefox/policies/policies.json`: the locked startup homepage and the
  `Contest Site` toolbar bookmark use `https://<gateway_hostname>/`. Existing
  notification allowances for the previous managed Gateway or legacy `domjudge`
  are updated; other policies, bookmarks and CA trust settings are preserved.
- `/etc/natsume/submit.env`: the public shell assignment
  `SUBMITBASEURL='https://<gateway_hostname>/'`, generated as root:root 0644.
  The image's `submit` launcher reads it on each invocation. It contains no
  credentials and is not an additional deployment input.

Restart the Device Daemon after editing the Client config, then fully quit and
reopen Firefox. The files must be regular root-owned files, with root-owned real
parent directories, writable only by root or the root group. Missing managed
folders/files are created as root:root 0755/0644. Invalid inputs fail startup with
the affected path; neither deployment config nor CA files are rewritten.

The image supplies Firefox and its baseline policies, including CA trust. It must
not bake the event hostname or run a competing policy writer. These derived
settings do not contain contest credentials and are independent of teams Home.
The image also supplies the pinned DOMjudge submit script and its launcher; the
launcher uses the Local Origin CA for Requests and bypasses proxies for the
Gateway hostname. Caddy injects the current team's Basic credentials on `/api/*`.
