#!/bin/sh
# Run inside a guest as root; export only numeric resource measurements.
set -eu
exec python3 - "$@" <<'PY'
import json
import os
import pathlib
import pwd
import re
import subprocess
import sys
import time

if len(sys.argv) != 3 or sys.argv[1] not in ["waiting", "contest", "compile", "logo"]:
    raise SystemExit("Usage: c5-sample.sh waiting|contest|compile|logo SECONDS")
scene, seconds = sys.argv[1], int(sys.argv[2])
if os.geteuid() != 0 or not 1 <= seconds <= 600:
    raise SystemExit("root and a bounded duration are required")
units = {
    "daemon": "natsume-device-daemon.service",
    "helper": "natsume-privileged-helper.service",
    "caddy": "natsume-caddy.service",
    "waiting_user": f"user@{pwd.getpwnam('waiting').pw_uid}.service",
}
changed = []
for name, unit in units.items():
    status = subprocess.check_output(
        ["systemctl", "show", unit, "-p", "CPUAccounting", "-p", "MemoryAccounting"],
        text=True,
    )
    if "CPUAccounting=yes" not in status or "MemoryAccounting=yes" not in status:
        directory = pathlib.Path("/run/systemd/system") / (unit + ".d")
        directory.mkdir(exist_ok=True)
        (directory / "90-natsume-bench-accounting.conf").write_text(
            "[Service]\nCPUAccounting=yes\nMemoryAccounting=yes\n"
        )
        changed.append(name)
if changed:
    subprocess.run(["systemctl", "daemon-reload"], check=True)
root = pathlib.Path("/run/natsume-bench-c5")
root.mkdir(mode=0o700, exist_ok=True)
(root / (scene + "-metadata.json")).write_text(
    json.dumps(
        {
            "scene": scene,
            "duration_s": seconds,
            "accounting_changed_units": changed,
            "sample_interval_s": 1,
        }
    )
    + "\n"
)
page_size = os.sysconf("SC_PAGE_SIZE")
hz = os.sysconf("SC_CLK_TCK")


def cgroup(unit, user=False):
    command = ["systemctl", "show", unit, "-p", "ControlGroup", "--value"]
    if user:
        uid = pwd.getpwnam("teams").pw_uid
        command = [
            "runuser",
            "-u",
            "teams",
            "--",
            "env",
            f"XDG_RUNTIME_DIR=/run/user/{uid}",
            *command[:1],
            "--user",
            *command[1:],
        ]
    r = subprocess.run(command, capture_output=True, text=True)
    value = r.stdout.strip()
    return (
        pathlib.Path("/sys/fs/cgroup" + value)
        if r.returncode == 0 and value.startswith("/")
        else None
    )


def integer(path):
    try:
        return int(path.read_text())
    except (FileNotFoundError, ValueError, PermissionError):
        return None


def processes(group):
    ids = set()
    if group is None or not group.exists():
        return ids
    for p in group.rglob("cgroup.procs"):
        try:
            ids.update(int(s) for s in p.read_text().split())
        except FileNotFoundError:
            pass
    return ids


def proc(pid):
    try:
        base = pathlib.Path("/proc") / str(pid)
        fields = (base / "stat").read_text().rsplit(")", 1)[1].split()
        rss = int((base / "statm").read_text().split()[1]) * page_size
        cpu = (int(fields[11]) + int(fields[12])) * 1000000 // hz
        agent = (base / "exe").readlink().name == "natsume-session-agent"
        return cpu, rss, agent
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return None


def sample(name, group):
    if group is None or not group.exists():
        return {"name": name, "available": False}
    stats = {}
    try:
        stats = dict(
            line.split() for line in (group / "cpu.stat").read_text().splitlines()
        )
    except FileNotFoundError:
        pass
    ps = [p for pid in processes(group) if (p := proc(pid)) is not None]
    return {
        "name": name,
        "available": True,
        "cpu_usage_us": int(stats.get("usage_usec", 0)),
        "memory_current_bytes": integer(group / "memory.current"),
        "memory_peak_bytes": integer(group / "memory.peak"),
        "rss_bytes": sum(p[1] for p in ps),
        "pid_count": len(ps),
    }


groups = {name: cgroup(unit) for name, unit in units.items()}
if scene == "compile":
    groups["teams_load"] = cgroup("natsume-bench-c5-load.service", True)
started = time.monotonic()
with (root / (scene + ".jsonl")).open("w") as output:
    for index in range(seconds + 1):
        delay = started + index - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        at = time.monotonic_ns() // 1000
        records = [sample(name, path) for name, path in groups.items()]
        agents = [
            p
            for pid in processes(groups["waiting_user"])
            if (p := proc(pid)) is not None and p[2]
        ]
        records.append(
            {
                "name": "agent_process",
                "available": bool(agents),
                "cpu_usage_us": sum(p[0] for p in agents),
                "rss_bytes": sum(p[1] for p in agents),
                "pid_count": len(agents),
            }
        )
        output.write(
            json.dumps(
                {"scene": scene, "sample": index, "at_boot_us": at, "records": records},
                separators=(",", ":"),
            )
            + "\n"
        )
        output.flush()
print(json.dumps({"scene": scene, "samples": seconds + 1, "outcome": "completed"}))
PY
