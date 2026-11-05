#!/bin/sh
# Run on B. The administrator password stays in the local credential file.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import http.cookiejar
import json
import pathlib
import re
import ssl
import sys
import time
import urllib.error
import urllib.request
import uuid

if len(sys.argv) not in [3, 4] or not re.fullmatch("[0-9a-f-]{36}", sys.argv[1]):
    raise SystemExit(
        "Usage: target.sh DEVICE wait|state|reset_home|terminate_session|set_foreground [waiting|contest]"
    )
ident, kind = sys.argv[1:3]
if kind not in ["wait", "state", "reset_home", "terminate_session", "set_foreground"]:
    raise SystemExit("invalid action")
mode = sys.argv[3] if len(sys.argv) == 4 else None
if kind == "set_foreground" and mode not in ["waiting", "contest"]:
    raise SystemExit("foreground required")
base = pathlib.Path("@BENCH_AUTH_DIR@")
opener = urllib.request.build_opener(
    urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    urllib.request.HTTPSHandler(
        context=ssl.create_default_context(cafile=str(base / "control-ca.crt"))
    ),
)


def api(method, path, body=None):
    request = urllib.request.Request(
        "@BENCH_SERVER_URL@/api/v2" + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method=method,
    )
    try:
        with opener.open(request, timeout=15) as response:
            return json.load(response)
    except Exception:
        raise SystemExit("API request failed; details withheld") from None


api(
    "POST",
    "/session",
    {
        "login_name": "@BENCH_ADMIN_LOGIN@",
        "password": (base / "operator-password").read_text().rstrip("\n"),
    },
)


def state(d):
    s = d["session_control"]["actual"]
    h = d["home"]["actual"]
    return {
        "received_at_unix_ms": d["received_at_unix_ms"],
        "observed_monotonic_us": time.monotonic_ns() // 1000,
        "observed_at_us": time.time_ns() // 1000,
        "foreground": s["foreground"],
        "waiting_ready": s["waiting_ready"],
        "contest_ready": s["contest_ready"],
        "home_state": h["state"],
        "home_epoch": h["completed_reset_epoch"],
        "terminate_epoch": s["completed_terminate_epoch"],
    }


def ready(d):
    s = d["session_control"]
    a = s.get("actual") or {}
    b = d["binding"].get("actual") or {}
    h = d["home"]
    g = d["gateway"]
    return (
        d["connection_state"] == "active"
        and all(
            d[k]["status"] == "converged"
            for k in ["binding", "gateway", "session_control", "home"]
        )
        and a.get("session_state") == "running"
        and a.get("waiting_ready")
        and a.get("contest_ready")
        and (mode is None or a.get("foreground") == mode)
        and b.get("assignment_state") == "applied"
        and b.get("credential_state") == "applied"
        and h.get("actual", {}).get("state") == "steady"
        and g.get("actual", {}).get("state") == "ready"
    )


if kind not in ["wait", "state"]:
    action = {"kind": kind}
    if mode:
        action["foreground_target"] = mode
    operation = str(uuid.uuid4())
    before = time.time_ns() // 1000
    api(
        "POST",
        "/target-submissions",
        {
            "operation_id": operation,
            "scope": {"kind": "devices", "device_ids": [ident]},
            "action": action,
        },
    )
    print(
        json.dumps(
            {
                "operation_id": operation,
                "kind": kind,
                "before_us": before,
                "submitted_us": time.time_ns() // 1000,
            }
        ),
        flush=True,
    )
for _ in range(1200):
    d = api("GET", "/devices/" + ident + "/convergence")
    if kind == "state" or ready(d):
        print(
            json.dumps(
                {"outcome": "observed" if kind == "state" else "converged", **state(d)}
            )
        )
        break
    time.sleep(0.1)
else:
    raise SystemExit("convergence timeout")
PY
