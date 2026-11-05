#!/bin/sh
# Run as guest root; never export Home contents, configuration values or HTTP bodies.
set -eu
exec python3 - "$@" <<'PY'
import hashlib, json, pathlib, pwd, subprocess, sys, time, tomllib

state = pathlib.Path("/var/lib/natsume-privileged/home-reset")
lines = (state / "progress").read_text().splitlines()
epoch = int(lines[0])
phase = lines[1]
assert phase in ["draining", "prepared", "applied", "verified", "recovery_required"]
generations = state / "generations"
home = pathlib.Path(pwd.getpwnam("teams").pw_dir)
completion = json.loads(
    pathlib.Path("/var/lib/natsume/state/home-completion.json").read_text()
)
o = {
    "kind": "snapshot",
    "at_us": time.time_ns() // 1000,
    "monotonic_us": time.monotonic_ns() // 1000,
    "reset_epoch": epoch,
    "phase": phase,
    "completion_epoch": completion["completed_reset_epoch"],
    "generations_count": len(list(generations.iterdir())),
    "generated_data_absent": not (home / ".natsume-bench-c3").exists(),
    "residual_test_directories": sum(
        1 for _ in generations.glob("*/upper/.natsume-bench-c3")
    ),
    "permit_present": pathlib.Path("/run/natsume-privileged/contest-permit").exists(),
    "window_present": (state / "login-window").exists(),
}
for label, unit in [
    ("helper", "natsume-privileged-helper.service"),
    ("daemon", "natsume-device-daemon.service"),
    ("caddy", "natsume-caddy.service"),
]:
    r = subprocess.check_output(
        [
            "systemctl",
            "show",
            unit,
            "-p",
            "MainPID",
            "-p",
            "NRestarts",
            "-p",
            "ActiveEnterTimestampMonotonic",
            "-p",
            "ActiveState",
        ],
        text=True,
    )
    props = dict(x.split("=", 1) for x in r.splitlines() if "=" in x)
    o[label] = {
        k: (int(v) if v.isdigit() else v)
        for k, v in props.items()
        if k in ["MainPID", "NRestarts", "ActiveEnterTimestampMonotonic", "ActiveState"]
        and (
            v.isdigit()
            or v in ["active", "inactive", "activating", "deactivating", "failed"]
        )
    }
if len(sys.argv) > 1 and sys.argv[1] == "caddy":
    o["hashes"] = {}
    for label, p in [
        ("bootstrap", pathlib.Path("/etc/natsume/caddy/bootstrap.caddyfile")),
        ("runtime", pathlib.Path("/run/natsume/caddy.caddyfile")),
        ("mode", pathlib.Path("/run/natsume/caddy-mode.json")),
    ]:
        o["hashes"][label] = (
            hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None
        )
    p = pathlib.Path("/run/natsume/caddy-admin.sock")
    if p.exists():
        st = p.stat()
        o["socket"] = {
            "mode": st.st_mode & 0o777,
            "uid": st.st_uid,
            "gid": st.st_gid,
            "inode": st.st_ino,
        }
    with open("/etc/natsume/config.toml", "rb") as f:
        hostname = tomllib.load(f)["site"]["gateway_hostname"]
    r = subprocess.run(
        [
            "curl",
            "--noproxy",
            "*",
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            "--connect-timeout",
            "1",
            "--max-time",
            "2",
            "--cacert",
            "/etc/natsume/trust/local-origin-ca.crt",
            "--resolve",
            hostname + ":443:127.0.0.1",
            "https://" + hostname + "/",
        ],
        capture_output=True,
        text=True,
    )
    o["http_status"] = int(r.stdout) if r.stdout.isdigit() else 0
    o["http_result"] = {
        0: "completed",
        7: "connection_failed",
        28: "timeout",
        35: "tls_connect_error",
        60: "tls_verify_failed",
    }.get(r.returncode, "other_error")
print(json.dumps(o))
PY
