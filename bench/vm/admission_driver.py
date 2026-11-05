from bench_env import configured

import sys
import pathlib
import json
import datetime

sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from fault_driver import run, guest, target, collect
from network_driver import put

CFG = configured()

assert len(sys.argv) == 2
raw = pathlib.Path(sys.argv[1])
assert raw.is_dir() and (raw / "expectations.md").exists()
assert not (raw / "contest.jsonl").exists()
for name in ["fault-state.sh", "c3-write.sh", "admission-probe.sh"]:
    put(name, CFG.expand("@BENCH_VM_ROOT@/") + name)
    run(
        "B",
        CFG.expand(
            "sudo -n tee @BENCH_VM_ROOT@/cr67-install.sh >/dev/null <<'PUBLIC'\nchmod 755 /run/natsume-bench-"
        )
        + name
        + CFG.expand(
            "\nPUBLIC\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM1@ @BENCH_VM_ROOT@/cr67-install.sh @BENCH_VM_ROOT@/"
        )
        + name
        + " /run/natsume-bench-"
        + name,
    )
(raw / "contest.jsonl").write_text(target(CFG["VM1"], "set_foreground", "contest"))
before = json.loads(guest(CFG["VM1"], "/bin/sh /run/natsume-bench-fault-state.sh"))
(raw / "before.json").write_text(json.dumps(before) + "\n")
baseline = guest(CFG["VM1"], "/bin/sh /run/natsume-bench-admission-probe.sh snapshot")
(raw / "baseline-admission.jsonl").write_text(baseline)
b = json.loads(baseline)
assert (
    b["gate_result"] == "GATE_ALLOWED"
    and b["helper_home_ready"]
    and all(b["pam_hooks"].values())
)
(raw / "write.json").write_text(
    guest(
        CFG["VM1"],
        "runuser -u teams -- /bin/sh /run/natsume-bench-c3-write.sh small500k 2026110467",
    )
)
guest(
    CFG["VM1"],
    f"systemd-run --unit=natsume-bench-cr7 --property=RuntimeMaxSec=190 /bin/sh /run/natsume-bench-admission-probe.sh watch {before['reset_epoch']} /run/natsume-bench-cr7.jsonl >/dev/null 2>&1",
)
since = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
(raw / "reset.jsonl").write_text(target(CFG["VM1"], "reset_home"))
(raw / "after.json").write_text(
    guest(CFG["VM1"], "/bin/sh /run/natsume-bench-fault-state.sh")
)
(raw / "after-admission.jsonl").write_text(
    guest(CFG["VM1"], "/bin/sh /run/natsume-bench-admission-probe.sh snapshot")
)
(raw / "observer.jsonl").write_text(
    guest(
        CFG["VM1"],
        "python3 - <<'PUBLIC'\nimport pathlib\nprint(pathlib.Path('/run/natsume-bench-cr7.jsonl').read_text(),end='')\nPUBLIC",
    )
)
collect(raw, CFG["VM1"], since, "reset")
print("CR7_collected=true", flush=True)
