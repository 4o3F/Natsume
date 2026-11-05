#!/bin/sh
# Workstation controller; all VM data stays on B and only filtered events return.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
exec python3 - "$script_dir" "$@" <<'PY'
from bench_env import configured

CFG = configured()
from pathlib import Path
import datetime
import json
import shlex
import subprocess
import sys
import time

script_dir = Path(sys.argv[1])
assert len(sys.argv) == 4, "Usage: c6-run.sh C6_OUTPUT C2_OUTPUT"
c6 = Path(sys.argv[2])
c2 = Path(sys.argv[3])
c6.mkdir(parents=True, exist_ok=False)
c2.mkdir(parents=True, exist_ok=False)
ssh = [
    "ssh",
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=yes",
    "-o",
    "UpdateHostKeys=no",
    "-o",
    "ForwardAgent=no",
]
hosts = {"A": "@BENCH_A@", "B": "@BENCH_B@"}
device = "@BENCH_DEVICE2@"


def run(host, script):
    r = subprocess.run(
        ssh + [hosts[host], "bash -se"],
        input="set -euo pipefail\n" + script,
        text=True,
        capture_output=True,
    )
    if r.returncode:
        raise SystemExit("remote step failed; details withheld")
    return r.stdout


clock_code = 'import sys,time,json\nfor line in sys.stdin:print(json.dumps({"monotonic_us":time.monotonic_ns()//1000,"realtime_us":time.time_ns()//1000}),flush=True)'
clocks = {
    name: subprocess.Popen(
        ssh + [address, shlex.join(["python3", "-u", "-c", clock_code])],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    for name, address in hosts.items()
}


def calibrate():
    values = {}
    for name, p in clocks.items():
        points = []
        for _ in range(8):
            before = time.monotonic_ns() // 1000
            p.stdin.write("tick\n")
            p.stdin.flush()
            r = json.loads(p.stdout.readline())
            after = time.monotonic_ns() // 1000
            points.append(
                {"workstation_before_us": before, "workstation_after_us": after, **r}
            )
        values[name] = min(
            points, key=lambda r: r["workstation_after_us"] - r["workstation_before_us"]
        )
    return values


for name in ["c6-cycle.sh", "c6-probe.sh"]:
    run(
        "B",
        "sudo -n tee @BENCH_VM_ROOT@/"
        + name
        + " >/dev/null <<'PUBLIC_PAYLOAD'\n"
        + CFG.script(script_dir / name)
        + "PUBLIC_PAYLOAD\nsudo -n chmod 700 @BENCH_VM_ROOT@/"
        + name,
    )
run(
    "B",
    "sudo -n tee /run/systemd/system/natsume-vm-@BENCH_VM2@.service >/dev/null <<'PUBLIC_UNIT'\n[Unit]\nDescription=Natsume @BENCH_VM2@ boot benchmark\n[Service]\nType=exec\nExecStart=@BENCH_VM_ROOT@/run-vm.sh @BENCH_VM_ROOT@ @BENCH_VM2@\nPUBLIC_UNIT\nsudo -n systemctl daemon-reload",
)
deadline = time.monotonic() + 1700
shutdown = r"""
sudo -n python3 - <<'SHUTDOWN'
import socket,json,time,subprocess
s=socket.socket(socket.AF_UNIX);s.settimeout(10);s.connect('@BENCH_VM_ROOT@/@BENCH_VM2@/guest-agent.sock');f=s.makefile('rwb',buffering=0)
ident=time.time_ns();f.write(b'\xff'+json.dumps({'execute':'guest-sync-delimited','arguments':{'id':ident},'id':ident}).encode()+b'\n')
while True:
 r=json.loads(f.readline().lstrip(b'\xff'))
 if r.get('id')==ident:break
before=time.monotonic_ns()//1000;f.write(json.dumps({'execute':'guest-shutdown','arguments':{'mode':'powerdown'},'id':time.time_ns()}).encode()+b'\n');f.close();s.close()
for _ in range(600):
 if subprocess.check_output(['systemctl','show','natsume-vm-@BENCH_VM2@.service','-p','ActiveState','--value'],text=True).strip()=='inactive':break
 time.sleep(.1)
else:raise SystemExit('normal shutdown timeout')
print(json.dumps({'kind':'shutdown','before_monotonic_us':before,'finished_monotonic_us':time.monotonic_ns()//1000,'elapsed_us':time.monotonic_ns()//1000-before}))
SHUTDOWN
"""
try:
    for cycle in range(1, 11):
        if time.monotonic() > deadline:
            raise SystemExit("sampling budget reached")
        prefix = f"{cycle:02d}"
        (c6 / (prefix + "-host-clocks-before.json")).write_text(
            json.dumps(calibrate(), indent=2) + "\n"
        )
        since = datetime.datetime.fromtimestamp(
            time.time() - 2, datetime.timezone.utc
        ).strftime("%Y-%m-%d %H:%M:%S UTC")
        output = run("B", "sudo -n /bin/sh @BENCH_VM_ROOT@/c6-cycle.sh " + str(cycle))
        records = [json.loads(x) for x in output.splitlines()]
        (c6 / (prefix + "-host.jsonl")).write_text(
            "".join((json.dumps(r) + "\n" for r in records if r["kind"] != "switch"))
        )
        (c2 / (prefix + "-operations.jsonl")).write_text(
            "".join((json.dumps(r) + "\n" for r in records if r["kind"] == "switch"))
        )
        probe = run(
            "B",
            "sudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/c6-probe.sh",
        )
        (c6 / (prefix + "-guest-boot.jsonl")).write_text(probe)
        for source, args in [
            ("server", ["server", hosts["A"], since, device]),
            (
                "guest",
                [
                    "guest",
                    hosts["B"],
                    "@BENCH_VM_ROOT@",
                    "@BENCH_VM2@",
                    "1970-01-01 UTC",
                ],
            ),
        ]:
            with (c6 / (prefix + "-" + source + ".jsonl")).open("w") as f:
                r = subprocess.run(
                    ["sh", str(script_dir / "collect-events.sh"), *args],
                    stdout=f,
                    stderr=subprocess.PIPE,
                )
            if r.returncode:
                raise SystemExit("event collection failed; details withheld")
        (c6 / (prefix + "-host-clocks-after.json")).write_text(
            json.dumps(calibrate(), indent=2) + "\n"
        )
        (c6 / (prefix + "-shutdown.json")).write_text(run("B", shutdown))
        print(
            json.dumps({"cycle": cycle, "outcome": "collected", "switches": 10}),
            flush=True,
        )
finally:
    for p in clocks.values():
        p.stdin.close()
        p.wait()
run(
    "B",
    "sudo -n rm /run/systemd/system/natsume-vm-@BENCH_VM2@.service\nsudo -n systemctl daemon-reload",
)
PY
