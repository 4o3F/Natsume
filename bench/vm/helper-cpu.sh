#!/bin/sh
# Run as guest root; export numeric counters, thread names and syscall names only.
set -eu
exec python3 - "$@" <<'PY'
import collections
import json
import os
import pathlib
import re
import shutil
import subprocess
import time

unit = "natsume-privileged-helper.service"
pid = int(
    subprocess.check_output(["systemctl", "show", unit, "-p", "MainPID", "--value"])
)
assert pid > 1
cgroup = subprocess.check_output(
    ["systemctl", "show", unit, "-p", "ControlGroup", "--value"], text=True
).strip()
cg = pathlib.Path("/sys/fs/cgroup") / cgroup.lstrip("/")
hz = os.sysconf("SC_CLK_TCK")


def process(path):
    text = (path / "stat").read_text()
    end = text.rindex(")")
    name = text[text.index("(") + 1 : end]
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,16}", name):
        name = "other"
    stat = text[end + 2 :].split()
    status = (path / "status").read_text()
    counters = {
        k: int(v)
        for k, v in re.findall(
            r"^(voluntary_ctxt_switches|nonvoluntary_ctxt_switches):\s+(\d+)$",
            status,
            re.M,
        )
    }
    return {"name": name, "ticks": int(stat[11]) + int(stat[12]), **counters}


def snapshot():
    threads = {}
    for path in pathlib.Path(f"/proc/{pid}/task").iterdir():
        try:
            threads[int(path.name)] = process(path)
        except (FileNotFoundError, ProcessLookupError):
            pass
    values = dict(line.split() for line in (cg / "cpu.stat").read_text().splitlines())
    return {
        "threads": threads,
        "main": process(pathlib.Path(f"/proc/{pid}")),
        "cgroup_cpu_us": int(values["usage_usec"]),
        "monotonic_ns": time.monotonic_ns(),
    }


def delta(before, after, phase):
    rows = []
    for tid, a in after["threads"].items():
        if tid not in before["threads"]:
            continue
        b = before["threads"][tid]
        rows.append(
            {
                "thread_name": a["name"],
                "cpu_us": (a["ticks"] - b["ticks"]) * 1000000 // hz,
                **{
                    k: a[k] - b[k]
                    for k in ["voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"]
                },
            }
        )
    print(
        json.dumps(
            {
                "phase": phase,
                "elapsed_us": (after["monotonic_ns"] - before["monotonic_ns"]) // 1000,
                "main_cpu_us": (after["main"]["ticks"] - before["main"]["ticks"])
                * 1000000
                // hz,
                "cgroup_cpu_us": after["cgroup_cpu_us"] - before["cgroup_cpu_us"],
                "voluntary_ctxt_switches": after["main"]["voluntary_ctxt_switches"]
                - before["main"]["voluntary_ctxt_switches"],
                "nonvoluntary_ctxt_switches": after["main"][
                    "nonvoluntary_ctxt_switches"
                ]
                - before["main"]["nonvoluntary_ctxt_switches"],
                "threads": rows,
            }
        ),
        flush=True,
    )


print(
    json.dumps(
        {
            "phase": "metadata",
            "strace_available": shutil.which("strace") is not None,
            "perf_available": shutil.which("perf") is not None,
            "clock_ticks_per_second": hz,
        }
    ),
    flush=True,
)
before = snapshot()
time.sleep(30)
delta(before, snapshot(), "baseline")
if shutil.which("strace"):
    output = pathlib.Path("/run/natsume-bench-helper-strace-summary")
    bus = subprocess.Popen(
        [
            "timeout",
            "--signal=INT",
            "30",
            "dbus-monitor",
            "--system",
            "--profile",
            "type='method_call',interface='org.natsume.Privileged1'",
            "type='method_call',interface='org.natsume.Device1'",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    before = snapshot()
    result = subprocess.run(
        [
            "timeout",
            "--signal=INT",
            "--kill-after=5",
            "30",
            "strace",
            "-f",
            "-c",
            "-p",
            str(pid),
            "-o",
            str(output),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    delta(before, snapshot(), "strace")
    headers = bus.communicate()[0]
    allowed = {
        "QueryManagedSessions",
        "GetSessionUiSnapshot",
        "RenewSessionAgentLease",
        "ConfirmSessionPresentation",
        "PrepareSession",
        "VerifyHomeReset",
        "QueryHomeReset",
        "IsHomeReady",
    }
    counts = collections.Counter()
    for line in headers.splitlines():
        for name in allowed:
            if re.search(r"(?<![A-Za-z])" + name + r"(?![A-Za-z])", line):
                counts[name] += 1
    print(json.dumps({"phase": "bus", "method_counts": dict(counts)}))
    # strace -c never records arguments; discard headers and diagnostics too.
    for line in output.read_text().splitlines():
        m = re.fullmatch(
            r"\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+(\d+)\s+(?:(\d+)\s+)?([a-zA-Z_][a-zA-Z0-9_]*)\s*",
            line,
        )
        if not m or m[6] == "total":
            continue
        print(
            json.dumps(
                {
                    "phase": "syscall",
                    "syscall": m[6],
                    "percent": float(m[1]),
                    "seconds": float(m[2]),
                    "us_per_call": float(m[3]),
                    "calls": int(m[4]),
                    "errors": int(m[5] or 0),
                }
            )
        )
PY
