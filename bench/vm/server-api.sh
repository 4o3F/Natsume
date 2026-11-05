#!/bin/sh
# Run on B; credentials and complete API responses never leave this process.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import http.cookiejar, json, pathlib, re, ssl, sys, time, urllib.request, uuid

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
        with opener.open(req, timeout=10) as response:
            data = response.read()
            return json.loads(data) if data else None
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


def emit(o):
    print(
        json.dumps(
            {
                "observed_at_us": time.time_ns() // 1000,
                "observed_monotonic_us": time.monotonic_ns() // 1000,
                **o,
            }
        ),
        flush=True,
    )


def ident(s):
    assert str(uuid.UUID(s)) == s
    return s


ENUM = set(
    "enabled disabled revoked medium strong active offline awaiting_fresh_state waiting contest greeter other none unknown applied absent failed pending pending_apply converged reconciling drifted awaiting_actual running starting terminating ambiguous error steady resetting recovery_required ready blocked restoring open closed".split()
)


def selected(d):
    c = d["convergence"]
    out = {
        k: d[k]
        for k in ["device_id", "machine_hardware_id"]
        if str(uuid.UUID(d[k])) == d[k]
    }
    for k in ["evidence_quality", "state"]:
        if d.get(k) in ENUM:
            out[k] = d[k]
    for k in ["connection_state", "received_at_unix_ms"]:
        v = c.get(k)
        if v is None or type(v) is int or v in ENUM:
            out[k] = v
    for name in ["binding", "gateway", "session_control", "home"]:
        part = c[name]
        a = part.get("actual") or {}
        t = part.get("target") or {}
        if name == "home":
            t = {"reset_epoch": part.get("target_reset_epoch")}
        out[name] = {}
        for k in [
            "state",
            "foreground",
            "waiting_ready",
            "contest_ready",
            "completed_reset_epoch",
            "completed_terminate_epoch",
            "assignment_state",
            "credential_state",
            "session_state",
        ]:
            v = a.get(k)
            if type(v) in [bool, int] or isinstance(v, str) and v in ENUM:
                out[name][k] = v
        for k in ["reset_epoch", "terminate_epoch", "foreground_target"]:
            v = t.get(k)
            if type(v) is int or isinstance(v, str) and v in ENUM:
                out[name]["target_" + k] = v
        if part.get("status") in ENUM:
            out[name]["status"] = part["status"]
    return out


cmd = sys.argv[1]
if cmd in ["devices", "watch"]:
    seconds = int(sys.argv[2]) if cmd == "watch" else 0
    assert 0 <= seconds <= 900
    end = time.monotonic() + seconds
    previous = {}
    while True:
        try:
            rows = api("GET", "/devices")
        except SystemExit:
            if cmd != "watch":
                raise
            if time.monotonic() >= end:
                break
            time.sleep(0.2)
            continue
        assert isinstance(rows, list)
        for d in rows:
            o = selected(d)
            if previous.get(d["device_id"]) != o:
                emit(o)
                previous[d["device_id"]] = o
        if time.monotonic() >= end:
            break
        time.sleep(0.2)
elif cmd == "reviews":
    for r in api("GET", "/enrollment-reviews"):
        emit(
            {
                "review_id": ident(r["review_id"]),
                "machine_hardware_id": ident(r["machine_hardware_id"]),
                "evidence_quality": r["evidence_quality"]
                if r["evidence_quality"] in ["medium", "strong"]
                else "unknown",
            }
        )
elif cmd == "window":
    assert sys.argv[2] in ["open", "closed"]
    emit(
        {"window": api("PUT", "/provisioning-window", {"state": sys.argv[2]})["state"]}
    )
elif cmd == "deny":
    api("POST", "/enrollment-reviews/" + ident(sys.argv[2]) + "/actions/deny", {})
    emit({"result": "denied"})
elif cmd == "disable":
    device = ident(sys.argv[2])
    assert device not in ["@BENCH_DEVICE1@", "@BENCH_DEVICE2@"]
    api("PATCH", "/devices/" + device, {"state": "disabled"})
    assert api("GET", "/devices/" + device)["state"] == "disabled"
    emit({"device_id": device, "state": "disabled"})
elif cmd == "lifecycle":
    device = ident(sys.argv[2])
    state = sys.argv[3]
    assert device not in ["@BENCH_DEVICE1@", "@BENCH_DEVICE2@"] and state in [
        "enabled",
        "disabled",
        "revoked",
    ]
    before = time.time_ns() // 1000
    api("PATCH", "/devices/" + device, {"state": state})
    emit({"device_id": device, "state": state, "before_us": before})
elif cmd == "burst":
    device=ident(sys.argv[2]); count=int(sys.argv[3]); assert device not in ['@BENCH_DEVICE1@','@BENCH_DEVICE2@'] and 1<=count<=2000
    before=time.time_ns()//1000;accepted=0
    for i in range(count):
        result=api('POST','/target-submissions',{'operation_id':str(uuid.uuid4()),'scope':{'kind':'devices','device_ids':[device]},'action':{'kind':'set_foreground','foreground_target':'contest' if i%2 else 'waiting'}})
        accepted+=sum(row.get('rejection') is None for row in result['results'])
    emit({'before_us':before,'submissions':count,'accepted':accepted})
elif cmd == "operation_count":
    operation = ident(sys.argv[2])
    d = api("GET", "/journal/events?operation_id=" + operation + "&limit=1")
    emit(
        {
            "operation_id": operation,
            "retained_events": len(d["events"]),
            "has_more": d["has_more"],
        }
    )
elif cmd == "journal":
    d = api("GET", "/journal/status")
    out = {}

    def safe(o):
        result = {}
        for k, v in o.items():
            if isinstance(v, dict) and k in {
                "journal",
                "categories",
                "device",
                "audit",
                "auth",
            }:
                result[k] = safe(v)
            elif (v is None or type(v) in [int, bool]) and k in {
                "queue_depth",
                "write_failed",
                "shutdown_timeouts",
                "db_bytes",
                "wal_bytes",
                "shm_bytes",
                "free_bytes",
                "paused",
                "dropped_total",
                "dropped_since",
                "oldest_retained_id",
                "oldest_retained_at",
                "server_utc_offset_minutes",
                "produced",
                "queued",
                "dropped",
                "delivered",
            }:
                result[k] = v
            elif k in {"status", "pause_reason", "last_error"} and v in {
                "ready",
                "paused",
                "degraded",
                "disk_budget",
                "open",
                "write",
                "filesystem",
                "maintenance",
                None,
            }:
                result[k] = v
        return result

    emit(safe(d))
elif cmd == "submit":
    device = ident(sys.argv[2])
    foreground = sys.argv[3]
    assert foreground in ["waiting", "contest"]
    operation = str(uuid.uuid4())
    before = time.time_ns() // 1000
    result = api(
        "POST",
        "/target-submissions",
        {
            "operation_id": operation,
            "scope": {"kind": "devices", "device_ids": [device]},
            "action": {"kind": "set_foreground", "foreground_target": foreground},
        },
    )
    assert result["operation_id"] == operation
    emit(
        {
            "operation_id": operation,
            "before_us": before,
            "foreground_target": foreground,
            "accepted": sum(r.get("rejection") is None for r in result["results"]),
        }
    )
elif cmd == "events":
    start = int(sys.argv[2])
    finish = int(sys.argv[3])
    page = api(
        "GET",
        f"/journal/events?from={start}&to={finish}&limit=1000&kind=lease_attached&kind=lease_replaced&kind=disconnected&kind=reconcile_retry&kind=lifecycle_changed&kind=journal_dropped&kind=journal_paused&kind=journal_resumed",
    )
    assert not page.get("has_more")
    reasons = set(
        "disabled revoked control_key_replaced ping_silence client_close transport_error protocol_violation actor_inbox_unavailable send_failed send_timeout outgoing_closed client_fault server_fault transient_limit outbound_backpressure outbound_closed server_shutdown actor_failed".split()
    )
    for r in page["events"]:
        if r.get("state") != "readable":
            continue
        h = r["header"]
        d = r["details"]
        kind = d.get("kind")
        if kind not in [
            "lease_attached",
            "lease_replaced",
            "disconnected",
            "reconcile_retry",
            "lifecycle_changed",
            "journal_dropped",
            "journal_paused",
            "journal_resumed",
        ]:
            continue
        o = {"event": kind, "occurred_at_ms": h["occurred_at_ms"]}
        if h.get("device_id"):
            o["device_id"] = ident(h["device_id"])
        if d.get("reason") in reasons:
            o["reason"] = d["reason"]
        if type(d.get("attempt")) is int:
            o["attempt"] = d["attempt"]
        for k in ["count", "since"]:
            if type(d.get(k)) is int:
                o[k] = d[k]
        if d.get("category") in ["device", "audit", "auth", "journal"]:
            o["category"] = d["category"]
        for k in ["from", "to"]:
            if d.get(k) in ["enabled", "disabled", "revoked"]:
                o[k] = d[k]
        emit(o)
else:
    raise SystemExit("invalid action")
PY
