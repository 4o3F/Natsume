#!/bin/sh
# Run as the contestant user in the active Home. Never print file contents or names.
set -eu
exec python3 - "$@" <<'PY'
import json
import os
import pathlib
import pwd
import random
import subprocess
import sys
import time

if len(sys.argv) != 3 or sys.argv[1] not in [
    "empty",
    "100MB",
    "1GB",
    "small",
    "combined",
    "small500k",
    "small1m",
]:
    raise SystemExit(
        "Usage: c3-write.sh empty|100MB|1GB|small|combined|small500k|small1m SEED"
    )
condition, seed = sys.argv[1], int(sys.argv[2])
account = pwd.getpwnam("teams")
if os.geteuid() != account.pw_uid:
    raise SystemExit("contestant identity is required")
home = pathlib.Path(account.pw_dir)
root = home / ".natsume-bench-c3"
if root.exists():
    raise SystemExit("previous test data must be reset before another write")
free = os.statvfs(home).f_bavail * os.statvfs(home).f_frsize
required = 12 * 1024**3 if condition in ["small500k", "small1m"] else 5 * 1024**3
if free < required:
    raise SystemExit("insufficient guest disk space")
started = time.monotonic_ns()
rng = random.Random(seed)
files = 0
logical = 0
if condition != "empty":
    root.mkdir(mode=0o700)
if condition in ["100MB", "1GB", "combined"]:
    remaining = 100000000 if condition == "100MB" else 1000000000
    with (root / "bulk.bin").open("xb") as output:
        while remaining:
            size = min(1048576, remaining)
            output.write(rng.randbytes(size))
            remaining -= size
            logical += size
    files += 1
if condition in ["small", "combined", "small500k", "small1m"]:
    directories = {"small500k": 5000, "small1m": 10000}.get(condition, 1000)
    for directory in range(directories):
        child = root / f"d{directory:04d}"
        child.mkdir()
        for number in range(100):
            size = rng.randint(1, 8) * 1024
            (child / f"f{number:03d}.bin").write_bytes(rng.randbytes(size))
            logical += size
            files += 1
os.sync()
elapsed = (time.monotonic_ns() - started) // 1000
allocated = (
    int(subprocess.check_output(["du", "-s", "-B1", str(root)], text=True).split()[0])
    if root.exists()
    else 0
)
actual_files = sum(p.is_file() for p in root.rglob("*")) if root.exists() else 0
home_du = subprocess.run(
    ["du", "-s", "-B1", str(home)], capture_output=True, text=True, check=True
)
home_bytes = int(home_du.stdout.split()[0])
print(
    json.dumps(
        {
            "seed": seed,
            "write_us": elapsed,
            "logical_bytes": logical,
            "du_bytes": allocated,
            "home_du_bytes": home_bytes,
            "file_count": actual_files,
            "expected_file_count": files,
            "guest_available_bytes": os.statvfs(home).f_bavail
            * os.statvfs(home).f_frsize,
        }
    )
)
if actual_files != files:
    raise SystemExit("generated file count mismatch")
PY
