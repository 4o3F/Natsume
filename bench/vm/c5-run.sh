#!/bin/sh
# Run C5 on the current @BENCH_VM2@ fixture; logo includes a supplemental five-minute window.
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
import sys, time, json, shlex, datetime, subprocess, concurrent.futures, uuid

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
if len(sys.argv) != 2:
    raise SystemExit("Usage: c5-run.sh OUTPUT")
raw = Path(sys.argv[1])
raw.mkdir(parents=True, exist_ok=False)
device = "@BENCH_DEVICE2@"


def guest(script):
    command = (
        "sudo -n tee @BENCH_VM_ROOT@/c5-probe.sh >/dev/null <<'PUBLIC_GUEST'\n"
        + script.rstrip()
        + "\nPUBLIC_GUEST\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/c5-probe.sh"
    )
    return run("B", command).stdout


def start(scene, seconds):
    return guest(
        f"systemd-run --quiet --unit=natsume-bench-c5-{scene} --property=UMask=0077 /bin/sh /run/natsume-bench-c5-sample.sh {scene} {seconds}\n"
    )


def collect(scene):
    code = r"""import pathlib,json
scene=SCENE
root=pathlib.Path('/run/natsume-bench-c5')
for line in (root/(scene+'.jsonl')).read_text().splitlines():
 r=json.loads(line)
 assert r['scene'] in ['waiting','contest','compile','logo']
 assert set(r)=={'scene','sample','at_boot_us','records'}
 for d in r['records']:
  assert d['name'] in ['daemon','helper','caddy','waiting_user','agent_process','teams_load']
  assert all(k in ['name','available','cpu_usage_us','memory_current_bytes','memory_peak_bytes','rss_bytes','pid_count'] for k in d)
  assert all(v is None or isinstance(v,(int,bool)) for k,v in d.items() if k!='name')
 print(json.dumps(r,separators=(',',':')))
""".replace("SCENE", repr(scene))
    out = guest("python3 - <<'RES_PY'\n" + code + "\nRES_PY\n")
    (raw / (scene + ".jsonl")).write_text(out)
    meta = guest(
        "python3 -c "
        + shlex.quote(
            "import pathlib,json; p=pathlib.Path('/run/natsume-bench-c5/"
            + scene
            + "-metadata.json');r=json.loads(p.read_text());print(json.dumps(r))"
        )
    )
    (raw / (scene + "-metadata.json")).write_text(meta)
    print(
        json.dumps(
            {"scene": scene, "samples": len(out.splitlines()), "outcome": "collected"}
        ),
        flush=True,
    )


for name in ["c5-sample.sh", "c5-load.sh"]:
    body = CFG.script(script_dir / name)
    run(
        "B",
        "sudo -n tee @BENCH_VM_ROOT@/"
        + name
        + " >/dev/null <<'PAYLOAD'\n"
        + body
        + "PAYLOAD\nsudo -n chmod 700 @BENCH_VM_ROOT@/"
        + name,
    )
target = CFG.script(script_dir / "target.sh")
run(
    "B",
    "umask 077; cat > @BENCH_RUN_ROOT@/c5-target.sh <<'TARGET'\n"
    + target
    + "TARGET\nchmod 700 @BENCH_RUN_ROOT@/c5-target.sh",
)
run(
    "B",
    r"""sudo -n sh -c 'umask 077; printf "%s\n" "true" > @BENCH_VM_ROOT@/c5-install.sh'
sudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/c5-install.sh @BENCH_VM_ROOT@/c5-sample.sh /run/natsume-bench-c5-sample.sh
sudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/c5-install.sh @BENCH_VM_ROOT@/c5-load.sh /run/natsume-bench-c5-load.sh""",
)
run("B", "/bin/sh @BENCH_RUN_ROOT@/c5-target.sh " + device + " set_foreground waiting")
start("waiting", 300)
time.sleep(303)
collect("waiting")
for scene, mode, seconds in [
    ("contest", "contest", 300),
    ("compile", "contest", 300),
    ("logo", "waiting", 600),
]:
    out = run(
        "B",
        "/bin/sh @BENCH_RUN_ROOT@/c5-target.sh " + device + " set_foreground " + mode,
    ).stdout
    (raw / (scene + "-target.jsonl")).write_text(out)
    if scene == "compile":
        print(guest("/bin/sh /run/natsume-bench-c5-load.sh start"), flush=True)
    if scene == "logo":
        print(guest("/bin/sh /run/natsume-bench-c5-load.sh stop"), flush=True)
        (raw / "logo-start.json").write_text(
            json.dumps(
                {
                    "started_utc": datetime.datetime.now(
                        datetime.timezone.utc
                    ).isoformat(),
                    "duration_s": seconds,
                }
            )
            + "\n"
        )
        code = (
            CFG.script(script_dir / "c5-logo.sh")
            .split("<<'PY'\n", 1)[1]
            .rsplit("\nPY", 1)[0]
        )
        backup = "@BENCH_SERVER_DATA@/c5-logo-" + uuid.uuid4().hex + ".bin"

        def update_logo():
            cmd = (
                shlex.join(
                    [
                        "sudo",
                        "-n",
                        "-u",
                        "natsume-server",
                        "python3",
                        "-",
                        "@BENCH_SERVER_LOGO@",
                        "20",
                        "30",
                        backup,
                    ]
                )
                + " <<'LOGO_PY'\n"
                + code
                + "\nLOGO_PY"
            )
            result = run("A", cmd, False)
            (raw / "logo-updates.jsonl").write_text(result.stdout)
            if result.returncode:
                raise SystemExit("logo updater failed")

        pool = concurrent.futures.ThreadPoolExecutor(max_workers=1)
        logo_future = pool.submit(update_logo)
    print(start(scene, seconds), flush=True)
    time.sleep(seconds + 3)
    collect(scene)
    if scene == "logo":
        logo_future.result()
        pool.shutdown()
PY
