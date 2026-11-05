#!/bin/sh
# Run in the guest; diagnostic values and HTTP responses are filtered before export.
set -eu
exec python3 - "$@" <<'PY'
import json, pathlib, subprocess, sys, time, tomllib

seconds = int(sys.argv[1])
output = sys.argv[2]
assert 1 <= seconds <= 900
with open("/etc/natsume/config.toml", "rb") as f:
    hostname = tomllib.load(f)["site"]["gateway_hostname"]
stages = set(
    "starting system_bus helper identity_preflight hardware_identity local_gateway control_identity reconciliation control_configuration connecting handshake pending_review authority_installation active".split()
)
statuses = set("starting retrying waiting_for_review denied active stopped".split())
codes = set(
    "CONNECT_TIMEOUT TCP_CONNECTION_REFUSED TCP_NETWORK_UNREACHABLE TCP_CONNECTION_FAILED TLS_CERTIFICATE_INVALID TLS_HANDSHAKE_FAILED WEBSOCKET_CONNECTION_FAILED ACTIVE_CONNECTION_ENDED LOCAL_DEACTIVATION_FAILED HANDSHAKE_TIMEOUT HANDSHAKE_DISCONNECTED".split()
)
root = pathlib.Path("/var/lib/natsume-privileged/home-reset")
last = {}
end = time.monotonic() + seconds
next_http = 0
with open(output, "w", buffering=1) as out:

    def emit(kind, fields):
        out.write(
            json.dumps(
                dict(
                    kind=kind,
                    at_us=time.time_ns() // 1000,
                    monotonic_us=time.monotonic_ns() // 1000,
                    **fields,
                )
            )
            + "\n"
        )

    while time.monotonic() < end:
        for label, name in [
            ("diagnostic", "latest.json"),
            ("failure", "last-failure.json"),
        ]:
            try:
                d = json.loads(
                    pathlib.Path("/run/natsume-diagnostics", name).read_text()
                )
                c = d.get("connection", {})
                fail = d.get("failure") or {}
                net = c.get("network", {})
                safe = {
                    k: d[k]
                    for k in ["stage", "status"]
                    if d.get(k) in (stages if k == "stage" else statuses)
                }
                safe.update(
                    {
                        k: d[k]
                        for k in ["sequence", "captured_at_boot_ms"]
                        if type(d.get(k)) is int
                    }
                )
                safe.update(
                    {
                        k: c[k]
                        for k in ["attempt", "attempt_duration_ms", "retry_delay_ms"]
                        if type(c.get(k)) is int
                    }
                )
                if fail.get("code") in codes:
                    safe["code"] = fail["code"]
                if net.get("carrier") in ["up", "down", "unknown"]:
                    safe["carrier"] = net["carrier"]
                if net.get("route") in [
                    "unknown",
                    "matching_observed_route",
                    "no_matching_observed_route",
                ]:
                    safe["route"] = net["route"]
                if safe != last.get(label):
                    emit(label, safe)
                    last[label] = safe
            except (OSError, ValueError, KeyError):
                pass
        if time.monotonic() >= next_http:
            next_http = time.monotonic() + 1
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
                    ".2",
                    "--max-time",
                    ".5",
                    "--cacert",
                    "/etc/natsume/trust/local-origin-ca.crt",
                    "--resolve",
                    hostname + ":443:127.0.0.1",
                    "https://" + hostname + "/",
                ],
                capture_output=True,
                text=True,
            )
            h = {
                "http_status": int(r.stdout) if r.stdout.isdigit() else 0,
                "http_result": {
                    0: "completed",
                    7: "connection_failed",
                    28: "timeout",
                    35: "tls_connect_error",
                    60: "tls_verify_failed",
                }.get(r.returncode, "other_error"),
            }
            try:
                ep, phase = (root / "progress").read_text().splitlines()
                h["reset_epoch"] = int(ep)
                if phase in [
                    "draining",
                    "prepared",
                    "applied",
                    "verified",
                    "recovery_required",
                ]:
                    h["phase"] = phase
                h["permit_present"] = pathlib.Path(
                    "/run/natsume-privileged/contest-permit"
                ).exists()
                h["window_present"] = (root / "login-window").exists()
                h["generations_count"] = len(list((root / "generations").iterdir()))
            except (OSError, ValueError):
                pass
            r = subprocess.run(
                ["loginctl", "show-seat", "seat0", "-p", "ActiveSession", "--value"],
                capture_output=True,
                text=True,
            )
            sid = r.stdout.strip()
            if sid and sid.isalnum():
                r = subprocess.run(
                    ["loginctl", "show-session", sid, "-p", "Name", "--value"],
                    capture_output=True,
                    text=True,
                )
                h["foreground"] = {
                    "teams": "contest",
                    "waiting": "waiting",
                    "gdm": "greeter",
                }.get(r.stdout.strip(), "other")
            emit("local", h)
        time.sleep(0.1)
PY
