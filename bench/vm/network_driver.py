"""Collect CR3/CR4 using source-filtered guest observations."""

from bench_env import configured

import datetime, json, pathlib, subprocess, sys, time

sys.dont_write_bytecode = True
from fault_driver import SSH, HOST, IDs, root, run, guest, target, collect

CFG = configured()


def put(name, dest):
    run(
        "B",
        "sudo -n tee "
        + dest
        + " >/dev/null <<'PUBLIC_NET'\n"
        + CFG.script(root / "bench/vm" / name)
        + "PUBLIC_NET\nsudo -n chmod 755 "
        + dest,
    )


def install(vm):
    put("network-clock.sh", CFG.expand("@BENCH_VM_ROOT@/network-clock.sh"))
    for name in ["network-probe.sh", "fault-state.sh"]:
        put(name, CFG.expand("@BENCH_VM_ROOT@/") + name)
        run(
            "B",
            CFG.expand(
                "sudo -n tee @BENCH_VM_ROOT@/net-install.sh >/dev/null <<'PUB'\nchmod 700 /run/natsume-bench-"
            )
            + name
            + CFG.expand("\nPUB\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ ")
            + vm
            + CFG.expand(" @BENCH_VM_ROOT@/net-install.sh @BENCH_VM_ROOT@/")
            + name
            + " /run/natsume-bench-"
            + name,
        )


def link(state):
    return run(
        "B",
        CFG.expand("sudo -n /bin/sh @BENCH_VM_ROOT@/network-link.sh @BENCH_VM2@ ")
        + state,
    ).stdout


def probe(prefix, seconds):
    guest(
        CFG["VM2"],
        f"systemd-run --unit=natsume-bench-net-{prefix} --property=RuntimeMaxSec={seconds + 5} /bin/sh /run/natsume-bench-network-probe.sh {seconds} /run/natsume-bench-net-{prefix}.jsonl >/dev/null 2>&1",
    )


def readprobe(prefix):
    return guest(
        CFG["VM2"],
        f"systemctl stop natsume-bench-net-{prefix}.service\npython3 - <<'PUBLIC_READ'\nimport pathlib\nprint(pathlib.Path('/run/natsume-bench-net-{prefix}.jsonl').read_text(),end='')\nPUBLIC_READ",
    )


def snapshot(raw, name):
    (raw / (name + ".json")).write_text(
        guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
    )
    (raw / (name + "-clock.json")).write_text(
        run("B", CFG.expand("sudo -n /bin/sh @BENCH_VM_ROOT@/network-clock.sh")).stdout
    )


def watch(raw, prefix, seconds):
    f = (raw / (prefix + "-api.jsonl")).open("w")
    p = subprocess.Popen(
        SSH
        + [
            HOST["B"],
            CFG.expand(
                f"/bin/sh @BENCH_RUN_ROOT@/fault-api.sh {IDs[CFG['VM2']]} {seconds}"
            ),
        ],
        stdout=f,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    return (p, f)


def shutdown():
    try:
        guest(CFG["VM2"], "systemctl poweroff --no-block")
    except RuntimeError:
        pass
    for _ in range(120):
        if (
            run(
                "B",
                CFG.expand("systemctl show natsume-vm-@BENCH_VM2@ -p MainPID --value"),
            ).stdout.strip()
            == "0"
        ):
            return
        time.sleep(0.1)
    raise RuntimeError("poweroff timeout")


def start():
    return run(
        "B",
        CFG.expand(
            "sudo -n systemd-run --unit=natsume-vm-@BENCH_VM2@ --property=Type=exec @BENCH_VM_ROOT@/run-vm.sh @BENCH_VM_ROOT@ @BENCH_VM2@ >/dev/null 2>&1\npython3 - <<'P'\nimport json,time\nprint(json.dumps({'kind':'vm_start','at_us':time.time_ns()//1000,'monotonic_us':time.monotonic_ns()//1000}))\nP"
        ),
    ).stdout


def waitqga():
    run(
        "B",
        CFG.expand(
            "sudo -n tee @BENCH_VM_ROOT@/net-qga-ready.sh >/dev/null <<'READY'\ntest -f /run/natsume-bench-network-probe.sh\nREADY"
        ),
    )
    for _ in range(80):
        r = run(
            "B",
            CFG.expand(
                "sudo -n timeout 2 @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/net-qga-ready.sh @BENCH_VM_ROOT@/network-probe.sh /run/natsume-bench-network-probe.sh"
            ),
            False,
        )
        if not r.returncode:
            return
        time.sleep(0.1)
    raise RuntimeError("QGA not ready")


def cr3(raw, trials=(("01-30s", 30), ("02-300s", 300))):
    install(CFG["VM2"])
    put("network-operations.sh", CFG.expand("@BENCH_RUN_ROOT@/network-operations.sh"))
    put("network-link.sh", CFG.expand("@BENCH_VM_ROOT@/network-link.sh"))
    put("fault-api.sh", CFG.expand("@BENCH_RUN_ROOT@/fault-api.sh"))
    for prefix, seconds in trials:
        if (raw / (prefix + "-before.json")).exists():
            raise RuntimeError("sample output already exists")
        (raw / (prefix + "-contest.jsonl")).write_text(
            target(CFG["VM2"], "set_foreground", "contest")
        )
        snapshot(raw, prefix + "-before")
        since = datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%d %H:%M:%S UTC"
        )
        probe(prefix, seconds + 100)
        p, f = watch(raw, prefix, seconds + 55)
        off = link("off")
        (raw / (prefix + "-link-off.jsonl")).write_text(off)
        print(
            json.dumps({"case": "CR3", "stage": "link_off", "seconds": seconds}),
            flush=True,
        )
        end = time.monotonic() + seconds
        request = None
        try:
            if seconds == 300:
                time.sleep(35)
                rf = (raw / (prefix + "-offline-target.jsonl")).open("w")
                request = subprocess.Popen(
                    SSH
                    + [
                        HOST["B"],
                        CFG.expand(
                            f"/bin/sh @BENCH_RUN_ROOT@/target.sh {IDs[CFG['VM2']]} set_foreground waiting"
                        ),
                    ],
                    stdout=rf,
                    stderr=subprocess.DEVNULL,
                )
            while time.monotonic() < end:
                time.sleep(min(1, end - time.monotonic()))
        finally:
            (raw / (prefix + "-link-on.jsonl")).write_text(link("on"))
        print(
            json.dumps({"case": "CR3", "stage": "link_on", "seconds": seconds}),
            flush=True,
        )
        time.sleep(45)
        (raw / (prefix + "-ready.jsonl")).write_text(target(CFG["VM2"], "state"))
        if request:
            request.wait(timeout=10)
            rf.close()
        (raw / (prefix + "-waiting.jsonl")).write_text(
            target(CFG["VM2"], "set_foreground", "waiting")
        )
        snapshot(raw, prefix + "-after")
        (raw / (prefix + "-probe.jsonl")).write_text(readprobe(prefix))
        collect(raw, CFG["VM2"], since, prefix)
        p.wait(timeout=70)
        f.close()
        start_ms = int(json.loads(off)["at_us"] // 1000) - 1000
        (raw / (prefix + "-operations.jsonl")).write_text(
            run(
                "B",
                CFG.expand(
                    f"/bin/sh @BENCH_RUN_ROOT@/network-operations.sh {IDs[CFG['VM2']]} {start_ms}"
                ),
            ).stdout
        )
        print(
            json.dumps({"case": "CR3", "stage": "collected", "seconds": seconds}),
            flush=True,
        )


def cr4(raw):
    install(CFG["VM2"])
    if (raw / "before.json").exists():
        raise RuntimeError("sample output already exists")
    (raw / "before-waiting.jsonl").write_text(
        target(CFG["VM2"], "set_foreground", "waiting")
    )
    snapshot(raw, "before")
    shutdown()
    since = datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%d %H:%M:%S UTC"
    )
    run("A", "sudo -n systemctl stop natsume-bench")
    (raw / "vm-start.jsonl").write_text(start())
    t = time.monotonic()
    waitqga()
    probe("cr4", 240)
    install(CFG["VM2"])
    print(json.dumps({"case": "CR4", "stage": "offline_boot_observing"}), flush=True)
    while time.monotonic() < t + 120:
        time.sleep(1)
    (raw / "offline.json").write_text(
        guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
    )
    (raw / "server-start.jsonl").write_text(
        run(
            "A",
            CFG.expand(
                r"""sudo -n systemctl start natsume-bench
python3 - <<'P'
import json,time,socket
for _ in range(100):
 s=socket.socket();r=s.connect_ex(('@BENCH_CONTROL_HOST@',@BENCH_CONTROL_PORT@));s.close()
 if r==0:break
 time.sleep(.05)
else:raise SystemExit('listener unavailable')
print(json.dumps({'kind':'server_start','at_us':time.time_ns()//1000,'monotonic_us':time.monotonic_ns()//1000}))
P"""
            ),
        ).stdout
    )
    p, f = watch(raw, "recovery", 65)
    (raw / "ready.jsonl").write_text(target(CFG["VM2"], "wait"))
    snapshot(raw, "after")
    (raw / "probe.jsonl").write_text(readprobe("cr4"))
    collect(raw, CFG["VM2"], since, "boot")
    p.wait(timeout=70)
    f.close()
    lower = int(
        datetime.datetime.strptime(since, "%Y-%m-%d %H:%M:%S UTC")
        .replace(tzinfo=datetime.timezone.utc)
        .timestamp()
        * 1000
    )
    (raw / "operations.jsonl").write_text(
        run(
            "B",
            CFG.expand(
                f"/bin/sh @BENCH_RUN_ROOT@/network-operations.sh {IDs[CFG['VM2']]} {lower}"
            ),
        ).stdout
    )
    print(json.dumps({"case": "CR4", "stage": "collected"}), flush=True)


def cr4_link(raw):
    install(CFG["VM2"])
    if (raw / "before.json").exists():
        raise RuntimeError("sample output already exists")
    (raw / "before-waiting.jsonl").write_text(
        target(CFG["VM2"], "set_foreground", "waiting")
    )
    snapshot(raw, "before")
    shutdown()
    since = datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%d %H:%M:%S UTC"
    )
    (raw / "vm-start.jsonl").write_text(start())
    for _ in range(20):
        try:
            off = link("off")
            break
        except RuntimeError:
            time.sleep(0.05)
    else:
        raise RuntimeError("monitor not ready")
    (raw / "link-off.jsonl").write_text(off)
    t = time.monotonic()
    p, f = watch(raw, "recovery", 185)
    try:
        waitqga()
        probe("cr4link", 240)
        install(CFG["VM2"])
        print(
            json.dumps({"case": "CR4-link", "stage": "offline_boot_observing"}),
            flush=True,
        )
        while time.monotonic() < t + 120:
            time.sleep(1)
        (raw / "offline.json").write_text(
            guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
        )
    finally:
        (raw / "link-on.jsonl").write_text(link("on"))
    (raw / "ready.jsonl").write_text(target(CFG["VM2"], "wait"))
    snapshot(raw, "after")
    (raw / "probe.jsonl").write_text(readprobe("cr4link"))
    collect(raw, CFG["VM2"], since, "boot")
    p.wait(timeout=70)
    f.close()
    lower = int(
        datetime.datetime.strptime(since, "%Y-%m-%d %H:%M:%S UTC")
        .replace(tzinfo=datetime.timezone.utc)
        .timestamp()
        * 1000
    )
    (raw / "operations.jsonl").write_text(
        run(
            "B",
            CFG.expand(
                f"/bin/sh @BENCH_RUN_ROOT@/network-operations.sh {IDs[CFG['VM2']]} {lower}"
            ),
        ).stdout
    )
    print(json.dumps({"case": "CR4-link", "stage": "collected"}), flush=True)


if __name__ == "__main__":
    case = sys.argv[1]
    raw = pathlib.Path(sys.argv[2])
    assert case in ["CR3", "CR4", "CR4-link"] and raw.is_dir()
    for name, dest in [
        ("network-link.sh", CFG.expand("@BENCH_VM_ROOT@/network-link.sh")),
        ("network-operations.sh", CFG.expand("@BENCH_RUN_ROOT@/network-operations.sh")),
        ("fault-api.sh", CFG.expand("@BENCH_RUN_ROOT@/fault-api.sh")),
    ]:
        put(name, dest)
    {"CR3": cr3, "CR4": cr4, "CR4-link": cr4_link}[case](raw)
