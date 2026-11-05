#!/bin/sh
# Run on B; keep credentials local and export selected convergence enums only.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import http.cookiejar, json, pathlib, ssl, sys, time, urllib.request

ident = sys.argv[1]
seconds = int(sys.argv[2])
assert len(ident) == 36 and 1 <= seconds <= 900
base = pathlib.Path("@BENCH_AUTH_DIR@")
opener = urllib.request.build_opener(
    urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    urllib.request.HTTPSHandler(
        context=ssl.create_default_context(cafile=str(base / "control-ca.crt"))
    ),
)


def api(method, path, body=None):
    req = urllib.request.Request(
        "@BENCH_SERVER_URL@/api/v2" + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method=method,
    )
    try:
        with opener.open(req, timeout=10) as r:
            return json.load(r)
    except Exception:
        return None


assert (
    api(
        "POST",
        "/session",
        {
            "login_name": "@BENCH_ADMIN_LOGIN@",
            "password": (base / "operator-password").read_text().rstrip("\n"),
        },
    )
    is not None
)
end = time.monotonic() + seconds
previous = None
while time.monotonic() < end:
    d = api("GET", "/devices/" + ident + "/convergence")
    if d is None:
        time.sleep(0.2)
        continue
    a = d["session_control"].get("actual") or {}
    h = d["home"].get("actual") or {}
    g = d["gateway"].get("actual") or {}
    b = d["binding"].get("actual") or {}
    current = {
        "connection_state": d.get("connection_state"),
        "foreground": a.get("foreground"),
        "waiting_ready": a.get("waiting_ready"),
        "contest_ready": a.get("contest_ready"),
        "home_state": h.get("state"),
        "home_epoch": h.get("completed_reset_epoch"),
        "gateway_state": g.get("state"),
        "gateway_status": d["gateway"].get("status"),
        "binding_assignment": b.get("assignment_state"),
        "binding_credential": b.get("credential_state"),
    }
    filtered = {
        k: v
        for k, v in current.items()
        if v is None
        or isinstance(v, (bool, int))
        or v
        in {
            "active",
            "offline",
            "pending",
            "disconnected",
            "unknown",
            "waiting",
            "contest",
            "greeter",
            "other",
            "none",
            "steady",
            "resetting",
            "recovery_required",
            "ready",
            "blocked",
            "restoring",
            "absent",
            "converged",
            "failed",
            "applied",
            "error",
            "applying",
            "registered",
            "review_pending",
            "awaiting_actual",
            "reconciling",
            "drifted",
        }
    }
    if filtered != previous:
        # All strings are closed API enums; never forward arbitrary response fields.
        allowed = {
            "active",
            "offline",
            "pending",
            "disconnected",
            "unknown",
            "waiting",
            "contest",
            "greeter",
            "other",
            "none",
            "steady",
            "resetting",
            "recovery_required",
            "ready",
            "blocked",
            "restoring",
            "absent",
            "converged",
            "pending_apply",
            "pending",
            "failed",
            "applied",
            "error",
            "applying",
            "registered",
            "review_pending",
            "awaiting_actual",
            "reconciling",
            "drifted",
        }
        current = filtered
        print(
            json.dumps(
                {
                    "observed_at_us": time.time_ns() // 1000,
                    "observed_monotonic_us": time.monotonic_ns() // 1000,
                    "received_at_unix_ms": d.get("received_at_unix_ms"),
                    **current,
                }
            ),
            flush=True,
        )
        previous = current
    time.sleep(0.2)
PY
