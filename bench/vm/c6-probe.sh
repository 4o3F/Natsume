#!/bin/sh
# Run inside the guest after readiness; export boot timings and sanitized unit chains.
set -eu
exec python3 - <<'PY'
import hashlib
import json
import pathlib
import re
import subprocess
import time

print(
    json.dumps(
        {
            "kind": "clock",
            "guest_monotonic_us": time.monotonic_ns() // 1000,
            "guest_realtime_us": time.time_ns() // 1000,
            "boot_id_hash": hashlib.sha256(
                pathlib.Path("/proc/sys/kernel/random/boot_id").read_bytes()
            ).hexdigest()[:16],
        }
    )
)
units = [
    "systemd-udev-settle.service",
    "natsume-privileged-helper.service",
    "natsume-device-daemon.service",
    "natsume-caddy.service",
    "gdm.service",
    "graphical.target",
]
properties = [
    "ActiveEnterTimestampMonotonic",
    "InactiveExitTimestampMonotonic",
    "ExecMainStartTimestampMonotonic",
    "ExecMainExitTimestampMonotonic",
]
for unit in units:
    r = subprocess.run(
        ["systemctl", "show", unit] + ["--property=" + p for p in properties],
        capture_output=True,
        text=True,
        check=True,
    )
    values = {
        k: int(v)
        for line in r.stdout.splitlines()
        for k, _, v in [line.partition("=")]
        if k in properties and v.isdigit()
    }
    print(json.dumps({"kind": "unit", "unit": unit, **values}))
manager = [
    "FirmwareTimestampMonotonic",
    "LoaderTimestampMonotonic",
    "KernelTimestampMonotonic",
    "InitRDTimestampMonotonic",
    "UserspaceTimestampMonotonic",
    "FinishTimestampMonotonic",
]
r = subprocess.run(
    ["systemctl", "show"] + ["--property=" + p for p in manager],
    capture_output=True,
    text=True,
    check=True,
)
print(
    json.dumps(
        {
            "kind": "manager",
            **{
                k: int(v)
                for line in r.stdout.splitlines()
                for k, _, v in [line.partition("=")]
                if k in manager and v.isdigit()
            },
        }
    )
)


def microseconds(value):
    factors = {
        "h": 3600000000,
        "min": 60000000,
        "s": 1000000,
        "ms": 1000,
        "us": 1,
        "µs": 1,
    }
    return int(
        sum(
            float(n) * factors[u]
            for n, u in re.findall(r"([0-9.]+)\s*(min|ms|us|µs|h|s)", value)
        )
    )


r = subprocess.run(["systemd-analyze", "time"], capture_output=True, text=True)
for value, phase in re.findall(
    r"([0-9. hminsuµ]+)\s+\((firmware|loader|kernel|initrd|userspace)\)", r.stdout
):
    print(
        json.dumps(
            {"kind": "analyze_time", "phase": phase, "elapsed_us": microseconds(value)}
        )
    )
for target in [
    "graphical.target",
    "natsume-device-daemon.service",
    "gdm.service",
    "systemd-udev-settle.service",
]:
    r = subprocess.run(
        ["systemd-analyze", "critical-chain", target, "--no-pager"],
        capture_output=True,
        text=True,
    )
    for line in r.stdout.splitlines():
        line = re.sub(r"\x1b\[[0-9;]*m", "", line)
        m = re.search(
            r"([A-Za-z0-9_.@:-]+\.(?:service|target))\s+@([0-9. hminsuµ]+?)(?:\s+\+([0-9. hminsuµ]+))?$",
            line,
        )
        if m:
            print(
                json.dumps(
                    {
                        "kind": "critical_chain",
                        "target": target,
                        "unit": m[1],
                        "at_us": microseconds(m[2]),
                        "duration_us": microseconds(m[3] or ""),
                    }
                )
            )
r = subprocess.run(
    ["timedatectl", "show", "--property=NTP", "--property=NTPSynchronized"],
    capture_output=True,
    text=True,
)
print(
    json.dumps(
        {
            "kind": "time_sync",
            **{
                k: v == "yes"
                for line in r.stdout.splitlines()
                for k, _, v in [line.partition("=")]
                if k in ["NTP", "NTPSynchronized"] and v in ["yes", "no"]
            },
        }
    )
)
PY
