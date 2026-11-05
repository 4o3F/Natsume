#!/bin/sh
# On B as root: switch the fixed QEMU net0 link and export host timestamps only.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
exec python3 - "$@" <<'PY'
import json, socket, sys, time

vm, state = sys.argv[1:]
assert vm in ["@BENCH_VM1@", "@BENCH_VM2@"] and state in ["off", "on"]
s = socket.socket(socket.AF_UNIX)
s.settimeout(2)
s.connect("@BENCH_VM_ROOT@/" + vm + "/monitor.sock")
s.recv(65536)
before = time.monotonic_ns() // 1000
at = time.time_ns() // 1000
s.sendall(("set_link net0 " + state + "\n").encode())
reply = b""
while b"(qemu)" not in reply:
    reply += s.recv(65536)
assert b"Error" not in reply and b"not found" not in reply
print(
    json.dumps(
        {
            "kind": "link",
            "state": state,
            "at_us": at,
            "monotonic_us": before,
            "completed_monotonic_us": time.monotonic_ns() // 1000,
            "result": "ok",
        }
    )
)
s.close()
PY
