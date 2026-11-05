#!/bin/sh
# Run on B as root after the guest observer is armed; no VM data leaves B.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import base64, json, pathlib, socket, subprocess, sys, time

output = pathlib.Path(sys.argv[1])
assert output.is_absolute()
s = socket.socket(socket.AF_UNIX)
s.settimeout(2)
s.connect("@BENCH_VM_ROOT@/@BENCH_VM1@/guest-agent.sock")
f = s.makefile("rwb", buffering=0)


def call(name, args, allow_error=False):
    ident = time.time_ns()
    f.write(
        json.dumps({"execute": name, "arguments": args, "id": ident}).encode() + b"\n"
    )
    while True:
        r = json.loads(f.readline().lstrip(b"\xff"))
        if r.get("id") == ident:
            if "error" in r:
                if allow_error:
                    return None
                raise RuntimeError("QGA operation rejected")
            return r["return"]


f.write(b"\xff")
call("guest-sync-delimited", {"id": time.time_ns()})
end = time.monotonic() + 120
while time.monotonic() < end:
    handle = call(
        "guest-file-open",
        {"path": "/run/natsume-bench-power-ready.json", "mode": "r"},
        True,
    )
    if handle is not None:
        data = call("guest-file-read", {"handle": handle, "count": 4096})
        call("guest-file-close", {"handle": handle})
        trigger = json.loads(base64.b64decode(data["buf-b64"]))
        break
    time.sleep(0.1)
else:
    raise SystemExit("power injection trigger timeout")
handle = call(
    "guest-file-open", {"path": "/run/natsume-bench-cr2-observer.jsonl", "mode": "r"}
)
data = call("guest-file-read", {"handle": handle, "count": 16384})
call("guest-file-close", {"handle": handle})
# These are records emitted by fault-home.sh, already limited to fixed fields.
records = [
    json.loads(x) for x in base64.b64decode(data["buf-b64"]).decode().splitlines()
]
output.write_text("".join(json.dumps(x) + "\n" for x in records))
f.close()
s.close()
before = time.monotonic_ns() // 1000
at = time.time_ns() // 1000
subprocess.run(
    ["systemctl", "kill", "--signal=SIGKILL", "natsume-vm-@BENCH_VM1@.service"],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    check=True,
)
for _ in range(200):
    if (
        subprocess.check_output(
            [
                "systemctl",
                "show",
                "natsume-vm-@BENCH_VM1@.service",
                "-p",
                "MainPID",
                "--value",
            ],
            text=True,
        ).strip()
        == "0"
    ):
        break
    time.sleep(0.05)
else:
    raise SystemExit("VM did not exit after kill")
r = subprocess.run(
    ["qemu-img", "check", "-q", "@BENCH_VM_ROOT@/@BENCH_VM1@/root.qcow2"], capture_output=True
)
record = {
    "kind": "power_kill",
    "at_us": at,
    "monotonic_us": before,
    "unit_inactive_monotonic_us": time.monotonic_ns() // 1000,
    "trigger": trigger,
    "qemu_img_check_code": r.returncode,
    "qemu_img_check": "passed" if r.returncode == 0 else "failed",
}
with output.with_suffix(".power.jsonl").open("w") as log:
    log.write(json.dumps(record) + "\n")
print(json.dumps(record), flush=True)
if r.returncode:
    raise SystemExit("overlay check failed; VM remains off")
subprocess.run(
    ["systemctl", "reset-failed", "natsume-vm-@BENCH_VM1@.service"],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
started = time.monotonic_ns() // 1000
subprocess.run(
    [
        "systemd-run",
        "--unit=natsume-vm-@BENCH_VM1@",
        "--property=Type=exec",
        "@BENCH_VM_ROOT@/run-vm.sh",
        "@BENCH_VM_ROOT@",
        "@BENCH_VM1@",
    ],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    check=True,
)
record = {"kind": "restart", "at_us": time.time_ns() // 1000, "monotonic_us": started}
with output.with_suffix(".power.jsonl").open("a") as log:
    log.write(json.dumps(record) + "\n")
print(json.dumps(record), flush=True)
PY
