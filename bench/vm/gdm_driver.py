from bench_env import configured

import sys
import pathlib
import datetime
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from fault_driver import run, guest, target, collect
from network_driver import put, start, install, watch, probe, readprobe

CFG = configured()

assert len(sys.argv) == 2
raw = pathlib.Path(sys.argv[1])
assert raw.is_dir() and (raw / "expectations.md").exists()
assert not (raw / "cold-vm-start.jsonl").exists()
put("gdm-timeout.sh", CFG.expand("@BENCH_VM_ROOT@/gdm-timeout.sh"))
run(
    "B",
    CFG.expand(
        "sudo -n tee @BENCH_VM_ROOT@/cr6-arm.sh >/dev/null <<'PUBLIC'\nsystemd-run --unit=natsume-bench-cr6-cold --property=RuntimeMaxSec=160 /bin/sh /run/natsume-bench-gdm-timeout.sh /run/natsume-bench-cr6-cold.jsonl >/dev/null 2>&1\nPUBLIC"
    ),
)
since = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
(raw / "cold-vm-start.jsonl").write_text(start())
for _ in range(100):
    r = run(
        "B",
        CFG.expand(
            "sudo -n timeout 2 @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/cr6-arm.sh @BENCH_VM_ROOT@/gdm-timeout.sh /run/natsume-bench-gdm-timeout.sh"
        ),
        False,
    )
    if r.returncode == 0:
        break
    time.sleep(0.1)
else:
    raise RuntimeError("QGA observer not armed")
print("CR6_cold_armed=true", flush=True)
install(CFG["VM2"])
probe("cr6cold", 150)
p, f = watch(raw, "cold", 130)
(raw / "cold-ready.jsonl").write_text(target(CFG["VM2"], "wait"))
(raw / "cold-after.json").write_text(
    guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
)
(raw / "cold-observer.jsonl").write_text(
    guest(
        CFG["VM2"],
        "python3 - <<'PUBLIC'\nimport pathlib\nprint(pathlib.Path('/run/natsume-bench-cr6-cold.jsonl').read_text(),end='')\nPUBLIC",
    )
)
(raw / "cold-local.jsonl").write_text(readprobe("cr6cold"))
collect(raw, CFG["VM2"], since, "cold")
p.wait(timeout=140)
f.close()
print("CR6_cold_collected=true", flush=True)
(raw / "warm-contest.jsonl").write_text(target(CFG["VM2"], "set_foreground", "contest"))
(raw / "warm-before.json").write_text(
    guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
)
guest(
    CFG["VM2"],
    "systemd-run --unit=natsume-bench-cr6-warm --property=RuntimeMaxSec=160 /bin/sh /run/natsume-bench-gdm-timeout.sh /run/natsume-bench-cr6-warm.jsonl >/dev/null 2>&1",
)
since = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
probe("cr6warm", 180)
p, f = watch(raw, "warm", 150)
(raw / "warm-terminate.jsonl").write_text(target(CFG["VM2"], "terminate_session"))
(raw / "warm-waiting.jsonl").write_text(target(CFG["VM2"], "set_foreground", "waiting"))
(raw / "warm-after.json").write_text(
    guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
)
(raw / "warm-observer.jsonl").write_text(
    guest(
        CFG["VM2"],
        "python3 - <<'PUBLIC'\nimport pathlib\nprint(pathlib.Path('/run/natsume-bench-cr6-warm.jsonl').read_text(),end='')\nPUBLIC",
    )
)
(raw / "warm-local.jsonl").write_text(readprobe("cr6warm"))
collect(raw, CFG["VM2"], since, "warm")
p.wait(timeout=160)
f.close()
print("CR6_warm_collected=true", flush=True)
