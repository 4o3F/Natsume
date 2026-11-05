"""Drive the current disposable workstation fixtures without exporting credentials."""

from bench_env import configured

import sys, json, pathlib, subprocess, time, shlex, datetime

CFG = configured()

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
HOST = {"A": CFG["A"], "B": CFG["B"]}
root = pathlib.Path(__file__).resolve().parents[2]


def run(host, script, check=True):
    result = subprocess.run(
        SSH + [HOST[host], "bash -se"],
        input="set -euo pipefail\n" + script,
        text=True,
        capture_output=True,
    )
    if check and result.returncode:
        raise RuntimeError("remote step failed; diagnostic output withheld")
    return result


IDs = {CFG["VM1"]: CFG["DEVICE1"], CFG["VM2"]: CFG["DEVICE2"]}


def guest(vm, script):
    r = run(
        "B",
        CFG.expand("sudo -n tee @BENCH_VM_ROOT@/cr-probe-")
        + vm
        + ".sh >/dev/null <<'PUBLIC_GUEST'\n"
        + script.rstrip()
        + CFG.expand(
            "\nPUBLIC_GUEST\nsudo -n env QGA_TIMEOUT_SECONDS=600 @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ "
        )
        + vm
        + CFG.expand(" @BENCH_VM_ROOT@/cr-probe-")
        + vm
        + ".sh",
        False,
    )
    if r.returncode:
        raise RuntimeError("guest step failed; details withheld")
    return r.stdout


def target(vm, action, mode=None):
    return run(
        "B",
        CFG.expand("/bin/sh @BENCH_RUN_ROOT@/target.sh ")
        + IDs[vm]
        + " "
        + action
        + (" " + mode if mode else ""),
    ).stdout


def collect(raw, vm, since, prefix):
    for source, args in [
        ("server", ["server", HOST["A"], since, IDs[vm]]),
        ("guest", ["guest", HOST["B"], CFG["VM_ROOT"], vm, since]),
    ]:
        r = subprocess.run(
            ["sh", str(root / "bench/vm/collect-events.sh"), *args],
            capture_output=True,
            text=True,
        )
        if r.returncode:
            raise RuntimeError("event collection failed")
        (raw / (prefix + "-" + source + ".jsonl")).write_text(r.stdout)
    r = subprocess.run(
        [
            "sh",
            str(root / "bench/vm/collect-retry.sh"),
            HOST["B"],
            CFG["VM_ROOT"],
            vm,
            since,
        ],
        capture_output=True,
        text=True,
    )
    if r.returncode:
        raise RuntimeError("retry collection failed")
    (raw / (prefix + "-retry.jsonl")).write_text(r.stdout)


def install_payloads():
    for name in ["fault-state.sh", "fault-home.sh", "c3-write.sh", "target.sh"]:
        p = root / "bench/vm" / name
        s = CFG.script(p)
        dest = (
            CFG.expand("@BENCH_RUN_ROOT@/target.sh")
            if name == "target.sh"
            else CFG.expand("@BENCH_VM_ROOT@/") + name
        )
        run(
            "B",
            "sudo -n tee "
            + dest
            + " >/dev/null <<'PUBLIC_PAYLOAD'\n"
            + s
            + "PUBLIC_PAYLOAD\nsudo -n chmod 755 "
            + dest,
        )
    for vm in [CFG["VM1"], CFG["VM2"]]:
        for name in ["fault-state.sh", "fault-home.sh", "c3-write.sh"]:
            run(
                "B",
                CFG.expand("sudo -n tee @BENCH_VM_ROOT@/cr-install-")
                + vm
                + ".sh >/dev/null <<'PUBLIC_INSTALL'\nchmod 700 /run/natsume-bench-"
                + name
                + CFG.expand(
                    "\nPUBLIC_INSTALL\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ "
                )
                + vm
                + CFG.expand(" @BENCH_VM_ROOT@/cr-install-")
                + vm
                + CFG.expand(".sh @BENCH_VM_ROOT@/")
                + name
                + " /run/natsume-bench-"
                + name,
            )
        guest(vm, "chmod 755 /run/natsume-bench-c3-write.sh")


def cr1(raw):
    for trial, delay in [(1, 3), (2, 7)]:
        prefix = f"{trial:02d}"
        (raw / (prefix + "-contest.jsonl")).write_text(
            target(CFG["VM1"], "set_foreground", "contest")
        )
        before = json.loads(
            guest(CFG["VM1"], "/bin/sh /run/natsume-bench-fault-state.sh")
        )
        (raw / (prefix + "-before.json")).write_text(json.dumps(before) + "\n")
        written = guest(
            CFG["VM1"],
            "runuser -u teams -- /bin/sh /run/natsume-bench-c3-write.sh small500k "
            + str(2026110410 + trial),
        )
        (raw / (prefix + "-write.json")).write_text(written)
        log = "/run/natsume-bench-cr1-" + prefix + ".jsonl"
        guest(
            CFG["VM1"],
            "systemd-run --unit=natsume-bench-cr1-"
            + prefix
            + " --property=RuntimeMaxSec=240 /bin/sh /run/natsume-bench-fault-home.sh helper "
            + str(delay)
            + " "
            + str(before["reset_epoch"])
            + " "
            + log
            + " >/dev/null 2>&1",
        )
        since = datetime.datetime.fromtimestamp(
            time.time() - 2, datetime.timezone.utc
        ).strftime("%Y-%m-%d %H:%M:%S UTC")
        op = target(CFG["VM1"], "reset_home")
        (raw / (prefix + "-reset.jsonl")).write_text(op)
        after = guest(CFG["VM1"], "/bin/sh /run/natsume-bench-fault-state.sh")
        (raw / (prefix + "-after.json")).write_text(after)
        obs = guest(
            CFG["VM1"],
            "systemctl stop natsume-bench-cr1-"
            + prefix
            + ".service\npython3 - <<'READ'\nimport pathlib\nprint(pathlib.Path("
            + repr(log)
            + ').read_text(),end="")\nREAD',
        )
        (raw / (prefix + "-observer.jsonl")).write_text(obs)
        collect(raw, CFG["VM1"], since, prefix)
        print(
            json.dumps(
                {
                    "case": "CR1",
                    "trial": trial,
                    "delay_s": delay,
                    "outcome": "collected",
                    "completion_epoch": json.loads(after)["completion_epoch"],
                }
            ),
            flush=True,
        )


def cr2(raw, trials=((1, 3), (2, 7))):
    s = CFG.script(root / "bench/vm/fault-power.sh")
    run(
        "B",
        CFG.expand(
            "sudo -n tee @BENCH_VM_ROOT@/fault-power.sh >/dev/null <<'PUBLIC_POWER'\n"
        )
        + s
        + CFG.expand(
            "PUBLIC_POWER\nsudo -n chmod 700 @BENCH_VM_ROOT@/fault-power.sh\nsudo -n install -d -m 700 @BENCH_VM_ROOT@/fault-logs"
        ),
    )
    for trial, delay in trials:
        prefix = f"{trial:02d}"
        (raw / (prefix + "-contest.jsonl")).write_text(
            target(CFG["VM1"], "set_foreground", "contest")
        )
        before = json.loads(
            guest(CFG["VM1"], "/bin/sh /run/natsume-bench-fault-state.sh")
        )
        (raw / (prefix + "-before.json")).write_text(json.dumps(before) + "\n")
        (raw / (prefix + "-write.json")).write_text(
            guest(
                CFG["VM1"],
                "runuser -u teams -- /bin/sh /run/natsume-bench-c3-write.sh small500k "
                + str(2026110420 + trial),
            )
        )
        guest(
            CFG["VM1"],
            "systemd-run --unit=natsume-bench-cr2 --property=RuntimeMaxSec=240 /bin/sh /run/natsume-bench-fault-home.sh power "
            + str(delay)
            + " "
            + str(before["reset_epoch"])
            + " /run/natsume-bench-cr2-observer.jsonl >/dev/null 2>&1",
        )
        since = datetime.datetime.fromtimestamp(
            time.time() - 2, datetime.timezone.utc
        ).strftime("%Y-%m-%d %H:%M:%S UTC")
        command = SSH + [
            HOST["B"],
            CFG.expand("/bin/sh @BENCH_RUN_ROOT@/target.sh ")
            + IDs[CFG["VM1"]]
            + " reset_home",
        ]
        request = subprocess.Popen(
            command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )
        power = run(
            "B",
            CFG.expand(
                "sudo -n /bin/sh @BENCH_VM_ROOT@/fault-power.sh @BENCH_VM_ROOT@/fault-logs/cr2-"
            )
            + prefix
            + ".jsonl",
        ).stdout
        (raw / (prefix + "-power.jsonl")).write_text(power)
        (raw / (prefix + "-observer-before.jsonl")).write_text(
            run(
                "B",
                CFG.expand("sudo -n cat @BENCH_VM_ROOT@/fault-logs/cr2-")
                + prefix
                + ".jsonl",
            ).stdout
        )
        run(
            "B",
            CFG.expand(
                "sudo -n tee @BENCH_VM_ROOT@/cr-install-@BENCH_VM1@.sh >/dev/null <<'INSTALL'\nchmod 700 /run/natsume-bench-fault-home.sh\nINSTALL"
            ),
        )
        for attempt in range(40):
            script = CFG.expand(
                "sudo -n timeout 2 @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM1@ @BENCH_VM_ROOT@/cr-install-@BENCH_VM1@.sh @BENCH_VM_ROOT@/fault-home.sh /run/natsume-bench-fault-home.sh"
            )
            r = run("B", script, False)
            if r.returncode == 0:
                break
            time.sleep(0.2)
        else:
            raise RuntimeError("QGA did not become ready")
        guest(
            CFG["VM1"],
            "systemd-run --unit=natsume-bench-cr2-after --property=RuntimeMaxSec=240 /bin/sh /run/natsume-bench-fault-home.sh observe 0 0 /run/natsume-bench-cr2-after.jsonl >/dev/null 2>&1",
        )
        for name in ["fault-state.sh", "c3-write.sh"]:
            run(
                "B",
                CFG.expand(
                    "sudo -n tee @BENCH_VM_ROOT@/cr-install-@BENCH_VM1@.sh >/dev/null <<'INSTALL'\nchmod 755 /run/natsume-bench-"
                )
                + name
                + CFG.expand(
                    "\nINSTALL\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM1@ @BENCH_VM_ROOT@/cr-install-@BENCH_VM1@.sh @BENCH_VM_ROOT@/"
                )
                + name
                + " /run/natsume-bench-"
                + name,
            )
        ready = target(CFG["VM1"], "wait")
        (raw / (prefix + "-ready.jsonl")).write_text(ready)
        stdout, stderr = request.communicate(timeout=10)
        (raw / (prefix + "-reset.jsonl")).write_text(stdout)
        (raw / (prefix + "-after.json")).write_text(
            guest(CFG["VM1"], "/bin/sh /run/natsume-bench-fault-state.sh")
        )
        (raw / (prefix + "-observer-after.jsonl")).write_text(
            guest(
                CFG["VM1"],
                "systemctl stop natsume-bench-cr2-after.service\npython3 - <<'READ'\nimport pathlib\nprint(pathlib.Path('/run/natsume-bench-cr2-after.jsonl').read_text(),end='')\nREAD",
            )
        )
        collect(raw, CFG["VM1"], since, prefix)
        print(
            json.dumps(
                {
                    "case": "CR2",
                    "trial": trial,
                    "delay_s": delay,
                    "outcome": "collected",
                    "submission_waiter_code": request.returncode,
                }
            ),
            flush=True,
        )


def cr5(raw):
    for name in ["fault-caddy.sh", "fault-api.sh"]:
        s = CFG.script(root / "bench/vm" / name)
        dest = (
            CFG.expand("@BENCH_VM_ROOT@/") + name
            if name == "fault-caddy.sh"
            else CFG.expand("@BENCH_RUN_ROOT@/") + name
        )
        run(
            "B",
            "sudo -n tee "
            + dest
            + " >/dev/null <<'PUBLIC_CR5'\n"
            + s
            + "PUBLIC_CR5\nsudo -n chmod 755 "
            + dest,
        )
    run(
        "B",
        CFG.expand(
            "sudo -n tee @BENCH_VM_ROOT@/cr5-install.sh >/dev/null <<'INSTALL'\nchmod 700 /run/natsume-bench-fault-caddy.sh\nINSTALL\nsudo -n @BENCH_VM_ROOT@/qga.sh @BENCH_VM_ROOT@ @BENCH_VM2@ @BENCH_VM_ROOT@/cr5-install.sh @BENCH_VM_ROOT@/fault-caddy.sh /run/natsume-bench-fault-caddy.sh"
        ),
    )
    for trial, mode in [(1, "crash"), (2, "crash"), (3, "crash"), (4, "admin")]:
        prefix = f"{trial:02d}-{mode}"
        (raw / (prefix + "-contest.jsonl")).write_text(
            target(CFG["VM2"], "set_foreground", "contest")
        )
        (raw / (prefix + "-before.json")).write_text(
            guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
        )
        since = datetime.datetime.fromtimestamp(
            time.time() - 2, datetime.timezone.utc
        ).strftime("%Y-%m-%d %H:%M:%S UTC")
        api_watch = subprocess.Popen(
            SSH
            + [
                HOST["B"],
                CFG.expand("/bin/sh @BENCH_RUN_ROOT@/fault-api.sh ")
                + IDs[CFG["VM2"]]
                + " 48",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        log = "/run/natsume-bench-cr5-" + str(trial) + ".jsonl"
        guest(
            CFG["VM2"],
            "systemd-run --unit=natsume-bench-cr5-"
            + str(trial)
            + " --property=RuntimeMaxSec=55 /bin/sh /run/natsume-bench-fault-caddy.sh "
            + mode
            + " "
            + log
            + " >/dev/null 2>&1",
        )
        if mode == "admin":
            time.sleep(1)
            (raw / (prefix + "-trigger.jsonl")).write_text(
                target(CFG["VM2"], "set_foreground", "waiting")
            )
        out, err = api_watch.communicate(timeout=65)
        assert api_watch.returncode == 0
        (raw / (prefix + "-api.jsonl")).write_text(out)
        (raw / (prefix + "-ready.jsonl")).write_text(
            target(CFG["VM2"], "set_foreground", "waiting")
        )
        (raw / (prefix + "-after.json")).write_text(
            guest(CFG["VM2"], "/bin/sh /run/natsume-bench-fault-state.sh caddy")
        )
        (raw / (prefix + "-watch.jsonl")).write_text(
            guest(
                CFG["VM2"],
                "python3 - <<'READ'\nimport pathlib\nprint(pathlib.Path("
                + repr(log)
                + ").read_text(),end='')\nREAD",
            )
        )
        collect(raw, CFG["VM2"], since, prefix)
        print(
            json.dumps(
                {"case": "CR5", "trial": trial, "mode": mode, "outcome": "collected"}
            ),
            flush=True,
        )


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ["CR1", "CR2", "CR2b", "CR5"]:
        raise SystemExit("Usage: fault-run.sh CR1|CR2|CR2b|CR5 NEW_OUTPUT")
    raw = pathlib.Path(sys.argv[2])
    raw.mkdir(parents=True, exist_ok=False)
    install_payloads()
    {"CR1": cr1, "CR2": cr2, "CR2b": lambda out: cr2(out, ((1, 5),)), "CR5": cr5}[
        sys.argv[1]
    ](raw)
