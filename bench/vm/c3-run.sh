#!/bin/sh
# Drive the current @BENCH_VM1@ fixture; public payloads travel through QGA.
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
import sys, json, datetime, subprocess, time, shlex

SSH = [
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
HOST = {"A": "@BENCH_A@", "B": "@BENCH_B@"}


def run(host, script, check=True):
    r = subprocess.run(
        SSH + [HOST[host], "bash -se"],
        input="set -euo pipefail\n" + script,
        text=True,
        capture_output=True,
    )
    if check and r.returncode:
        raise SystemExit("remote operation failed; diagnostic output withheld")
    return r


script_dir = Path(sys.argv.pop(1))
if len(sys.argv) not in [2, 3]:
    raise SystemExit("Usage: c3-run.sh OUTPUT [--combined|--large-small]")
if len(sys.argv) == 3 and sys.argv[2] not in ["--combined", "--large-small"]:
    raise SystemExit("invalid optional argument")
raw = Path(sys.argv[1])
raw.mkdir(parents=True, exist_ok=False)
device = "@BENCH_DEVICE1@"
target = CFG.script(script_dir / "target.sh")
run(
    "B",
    "umask 077; cat > @BENCH_RUN_ROOT@/c3-target.sh <<'TARGET_SH'\n"
    + target
    + "TARGET_SH\nchmod 700 @BENCH_RUN_ROOT@/c3-target.sh",
)
s = CFG.script(script_dir / "c3-write.sh")
run(
    "B",
    "sudo -n tee @BENCH_VM_ROOT@/c3-write.sh >/dev/null <<'PUBLIC_SH'\n"
    + s
    + "PUBLIC_SH\nsudo -n chmod 700 @BENCH_VM_ROOT@/c3-write.sh",
)
run(
    "B",
    'sudo -n sh -c \'umask 077; printf "%s\\n" "chmod 755 /run/natsume-bench-c3-write.sh" > @BENCH_VM_ROOT@/c3-install.sh\'\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM1@ @BENCH_VM_ROOT@/c3-install.sh @BENCH_VM_ROOT@/c3-write.sh /run/natsume-bench-c3-write.sh',
)


def guest(script):
    r = run(
        "B",
        "sudo -n tee @BENCH_VM_ROOT@/c3-probe.sh >/dev/null <<'PUBLIC_GUEST'\n"
        + script.rstrip()
        + "\nPUBLIC_GUEST\nsudo -n env QGA_TIMEOUT_SECONDS=600 @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM1@ @BENCH_VM_ROOT@/c3-probe.sh",
        False,
    )
    if r.returncode:
        raise SystemExit("guest data operation failed; diagnostic output withheld")
    return r.stdout


sequence = [("empty", 3), ("100MB", 5), ("1GB", 5), ("small", 5)]
large = len(sys.argv) == 3 and sys.argv[2] == "--large-small"
if large:
    sequence = [("small500k", 3), ("small1m", 3)]
if len(sys.argv) == 3 and sys.argv[2] == "--combined":
    sequence.append(("combined", 5))
deadline = time.monotonic() + 1200
index = 0
for condition, count in sequence:
    if condition == "combined" and time.monotonic() + 240 > deadline:
        print("combined_skipped_due_to_budget=true", flush=True)
        break
    for trial in range(1, count + 1):
        if condition == "small1m" and trial == 3 and (written["write_us"] > 180000000):
            print("million_third_skipped_due_to_write_time=true", flush=True)
            break
        index += 1
        name = f"{index:02d}-{condition}-{trial}"
        seed = 2026110400 + index
        if time.monotonic() > deadline:
            raise SystemExit("C3 time budget reached")
        out = run(
            "B",
            "/bin/sh @BENCH_RUN_ROOT@/c3-target.sh "
            + device
            + " set_foreground contest",
        ).stdout
        (raw / (name + "-contest.jsonl")).write_text(out)
        written = json.loads(
            guest(
                "runuser -u teams -- /bin/sh /run/natsume-bench-c3-write.sh "
                + condition
                + " "
                + str(seed)
            )
        )
        (raw / (name + "-write.json")).write_text(
            json.dumps({"condition": condition, "trial": trial, **written}) + "\n"
        )
        since = datetime.datetime.fromtimestamp(
            time.time() - 2, datetime.timezone.utc
        ).strftime("%Y-%m-%d %H:%M:%S UTC")
        out = run(
            "B", "/bin/sh @BENCH_RUN_ROOT@/c3-target.sh " + device + " reset_home"
        ).stdout
        (raw / (name + "-reset.jsonl")).write_text(out)
        clean = guest(
            r"""python3 - <<'CLEAN_PY'
import pathlib, pwd, json
home = pathlib.Path(pwd.getpwnam('teams').pw_dir)
generations = pathlib.Path('/var/lib/natsume-privileged/home-reset/generations')
result = {
    'generated_data_absent': not (home / '.natsume-bench-c3').exists(),
    'generations_count': len(list(generations.iterdir())),
    'residual_test_directories': sum(1 for _ in generations.glob('*/upper/.natsume-bench-c3')),
}
print(json.dumps(result))
assert result['generated_data_absent']
assert result['generations_count'] == 1
assert result['residual_test_directories'] == 0
CLEAN_PY
"""
        )
        (raw / (name + "-clean.json")).write_text(clean)
        for source, args in [
            ("server", ["server", "@BENCH_A@", since, device]),
            ("guest", ["@BENCH_B@", "@BENCH_VM_ROOT@", "@BENCH_VM1@", since]),
        ]:
            script = str(
                script_dir
                / ("collect-events.sh" if source == "server" else "collect-home.sh")
            )
            with (raw / (name + "-" + source + ".jsonl")).open("w") as f:
                r = subprocess.run(
                    ["sh", script, *args], stdout=f, stderr=subprocess.PIPE
                )
            if r.returncode:
                raise SystemExit("Home event collection failed; details withheld")
        if large:
            with (raw / (name + "-retry.jsonl")).open("w") as f:
                subprocess.run(
                    [
                        "sh",
                        str(script_dir / "collect-retry.sh"),
                        "@BENCH_B@",
                        "@BENCH_VM_ROOT@",
                        "@BENCH_VM1@",
                        since,
                    ],
                    stdout=f,
                    check=True,
                )
        print(
            json.dumps(
                {
                    "condition": condition,
                    "trial": trial,
                    "outcome": "collected",
                    "write_us": written["write_us"],
                    "du_bytes": written["du_bytes"],
                    "file_count": written["file_count"],
                }
            ),
            flush=True,
        )
        time.sleep(1)
PY
