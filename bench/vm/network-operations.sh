#!/bin/sh
# Run on B; export only fixed connection event kinds, reasons and occurrence times.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import http.cookiejar, json, pathlib, ssl, sys, time, urllib.request

ident = sys.argv[1]
assert len(ident) == 36 and sys.argv[2].isdigit()
assert len(sys.argv) in [3, 4] and (len(sys.argv) == 3 or sys.argv[3].isdigit())
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

page = api(
    "GET",
    "/journal/events?device_id="
    + ident
    + "&from="
    + sys.argv[2]
    + "&to="
    + (sys.argv[3] if len(sys.argv) > 3 else str(time.time_ns() // 1000000))
    + "&limit=1000&kind=disconnected&kind=lease_attached&kind=lease_replaced",
)
assert page is not None and "events" in page
for r in page["events"]:
    if r.get("state") != "readable":
        continue
    h = r["header"]
    d = r["details"]
    kind = d.get("kind")
    if kind not in ["disconnected", "lease_attached", "lease_replaced"]:
        continue
    out = {"event": kind, "occurred_at_ms": h["occurred_at_ms"]}
    if d.get("reason") in [
        "disabled",
        "revoked",
        "control_key_replaced",
        "ping_silence",
        "client_close",
        "transport_error",
        "protocol_violation",
        "actor_inbox_unavailable",
        "send_failed",
        "send_timeout",
        "outgoing_closed",
        "client_fault",
        "server_fault",
        "transient_limit",
        "outbound_backpressure",
        "outbound_closed",
        "server_shutdown",
        "actor_failed",
    ]:
        out["reason"] = d["reason"]
    print(json.dumps(out))
assert not page.get("has_more"), "event window requires pagination"
PY
