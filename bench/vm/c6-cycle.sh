#!/bin/sh
# Run on B as root. Keep administrator credentials on B; emit selected metrics only.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import base64
import http.cookiejar
import json
import pathlib
import socket
import ssl
import subprocess
import sys
import time
import urllib.request
import uuid

cycle = int(sys.argv[1])
assert 1 <= cycle <= 10
work = pathlib.Path("@BENCH_VM_ROOT@")
vm = "@BENCH_VM2@"
device = "@BENCH_DEVICE2@"
unit = "natsume-vm-@BENCH_VM2@.service"
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
        with opener.open(req, timeout=15) as r:
            return json.load(r)
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


def emit(kind, **values):
    print(json.dumps({"kind": kind, "cycle": cycle, **values}), flush=True)


def wait(mode, after_ms, seconds=150):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        d = api("GET", "/devices/" + device + "/convergence")
        a = d["session_control"].get("actual") or {}
        h = d["home"].get("actual") or {}
        b = d["binding"].get("actual") or {}
        g = d["gateway"].get("actual") or {}
        if (
            d["connection_state"] == "active"
            and (d.get("received_at_unix_ms") or 0) >= after_ms
            and all(
                d[k]["status"] == "converged"
                for k in ["binding", "gateway", "home", "session_control"]
            )
            and a.get("foreground") == mode
            and a.get("waiting_ready")
            and a.get("contest_ready")
            and h.get("state") == "steady"
            and b.get("assignment_state") == "applied"
            and b.get("credential_state") == "applied"
            and g.get("state") == "ready"
        ):
            return {
                "received_at_unix_ms": d["received_at_unix_ms"],
                "home_epoch": h.get("completed_reset_epoch"),
                "terminate_epoch": a.get("completed_terminate_epoch"),
                "waiting_ready": True,
                "contest_ready": True,
                "foreground": mode,
            }
        time.sleep(0.1)
    raise SystemExit("convergence timeout; details withheld")


def qga(command, args):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(10)
    s.connect(str(work / vm / "guest-agent.sock"))
    f = s.makefile("rwb", buffering=0)

    def call(name, values):
        ident = time.time_ns()
        f.write(
            json.dumps({"execute": name, "arguments": values, "id": ident}).encode()
            + b"\n"
        )
        while True:
            r = json.loads(f.readline().lstrip(b"\xff"))
            if r.get("id") == ident:
                if "error" in r:
                    raise RuntimeError("QGA operation rejected")
                return r["return"]

    f.write(b"\xff")
    call("guest-sync-delimited", {"id": time.time_ns()})
    if command == "guest-shutdown":
        f.write(
            json.dumps(
                {"execute": command, "arguments": args, "id": time.time_ns()}
            ).encode()
            + b"\n"
        )
        result = None
    else:
        result = call(command, args)
    f.close()
    s.close()
    return result


assert (
    subprocess.check_output(
        ["systemctl", "show", unit, "-p", "ActiveState", "--value"], text=True
    ).strip()
    == "inactive"
)
start_realtime_us = time.time_ns() // 1000
start_monotonic_us = time.monotonic_ns() // 1000
subprocess.run(
    ["systemctl", "start", unit],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    check=True,
)
active_us = time.monotonic_ns() // 1000
props = subprocess.check_output(
    [
        "systemctl",
        "show",
        unit,
        "-p",
        "ExecMainStartTimestampMonotonic",
        "-p",
        "ActiveEnterTimestampMonotonic",
        "-p",
        "InactiveExitTimestampMonotonic",
        "-p",
        "MainPID",
    ],
    text=True,
)
host = {
    k: int(v)
    for line in props.splitlines()
    for k, _, v in [line.partition("=")]
    if v.isdigit()
}
pid = host["MainPID"]
stat = pathlib.Path(f"/proc/{pid}/stat").read_text()
fields = stat[stat.rindex(")") + 2 :].split()
import os

host["qemu_proc_start_us"] = int(fields[19]) * 1000000 // os.sysconf("SC_CLK_TCK")
emit(
    "host_start",
    start_realtime_us=start_realtime_us,
    start_monotonic_us=start_monotonic_us,
    start_return_monotonic_us=active_us,
    **host,
)
ready = wait("waiting", start_realtime_us // 1000 - 50)
ready_seen = time.monotonic_ns() // 1000
emit("boot_ready", observed_monotonic_us=ready_seen, **ready)
for roundtrip in range(5):
    for mode in ["contest", "waiting"]:
        operation = str(uuid.uuid4())
        before = time.time_ns() // 1000
        before_mono = time.monotonic_ns() // 1000
        api(
            "POST",
            "/target-submissions",
            {
                "operation_id": operation,
                "scope": {"kind": "devices", "device_ids": [device]},
                "action": {"kind": "set_foreground", "foreground_target": mode},
            },
        )
        submitted = time.time_ns() // 1000
        final = wait(mode, before // 1000 - 50)
        emit(
            "switch",
            roundtrip=roundtrip,
            temperature="first_after_boot" if roundtrip == 0 else "hot",
            mode=mode,
            operation_id=operation,
            before_us=before,
            submitted_us=submitted,
            before_monotonic_us=before_mono,
            observed_monotonic_us=time.monotonic_ns() // 1000,
            ready_to_first_submit_us=before_mono - ready_seen
            if roundtrip == 0 and mode == "contest"
            else None,
            **final,
        )
        time.sleep(3)
# Timestamp the guest clock against B's monotonic around a small QGA exec.
probes = []
for _ in range(5):
    before = time.monotonic_ns() // 1000
    pid = qga(
        "guest-exec",
        {
            "path": "/usr/bin/python3",
            "arg": [
                "-c",
                'import time,json; print(json.dumps({"guest_monotonic_us":time.monotonic_ns()//1000,"guest_realtime_us":time.time_ns()//1000}))',
            ],
            "capture-output": True,
        },
    )["pid"]
    while True:
        r = qga("guest-exec-status", {"pid": pid})
        if r.get("exited"):
            break
        time.sleep(0.005)
    after = time.monotonic_ns() // 1000
    assert not r.get("out-truncated") and r.get("exitcode", 0) == 0
    value = json.loads(base64.b64decode(r["out-data"]))
    probes.append({"B_before_us": before, "B_after_us": after, **value})
emit("guest_clock", probes=probes)
# Do not close the VM until the workstation has taken the source-filtered journals.
emit("await_collection")
PY
