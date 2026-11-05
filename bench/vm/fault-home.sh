#!/bin/sh
# Guest observer/injector. Public log contains fixed Home fields and numeric counters only.
set -eu
exec python3 - "$@" <<'PY'
import json, os, pathlib, signal, subprocess, sys, time

mode, delay, minimum, output = sys.argv[1:]
delay = float(delay)
minimum = int(minimum)
assert mode in ["helper", "power", "observe"] and delay in [0, 3, 5, 7]
root = pathlib.Path("/var/lib/natsume-privileged/home-reset")
end = time.monotonic() + 240
previous = None
applied = None
injected = False
with open(output, "w", buffering=1) as out:
    while time.monotonic() < end:
        try:
            epoch, phase = (root / "progress").read_text().splitlines()
            epoch = int(epoch)
            assert phase in [
                "draining",
                "prepared",
                "applied",
                "verified",
                "recovery_required",
            ]
            generations = list((root / "generations").iterdir())
            r = {
                "reset_epoch": epoch,
                "phase": phase,
                "generations_count": len(generations),
                "permit_present": pathlib.Path(
                    "/run/natsume-privileged/contest-permit"
                ).exists(),
                "window_present": (root / "login-window").exists(),
            }
        except (OSError, ValueError, AssertionError):
            time.sleep(0.1)
            continue
        if r != previous:
            out.write(
                json.dumps(
                    {
                        "kind": "state",
                        "at_us": time.time_ns() // 1000,
                        "monotonic_us": time.monotonic_ns() // 1000,
                        **r,
                    }
                )
                + "\n"
            )
            previous = r
        eligible = (
            epoch > minimum
            and phase == "applied"
            and len(generations) == 2
            and not r["permit_present"]
            and r["window_present"]
        )
        if eligible and applied is None:
            applied = time.monotonic_ns() // 1000
        if (
            mode != "observe"
            and eligible
            and not injected
            and time.monotonic_ns() // 1000 - applied >= delay * 1e6
        ):
            remaining = 0
            for generation in generations:
                if generation.name != str(epoch):
                    p = generation / "upper/.natsume-bench-c3"
                    if p.exists():
                        remaining += sum(1 for _ in p.iterdir())
            assert 0 < remaining < 5000, "deletion must already be progressing"
            r = {
                "kind": "injection",
                "at_us": time.time_ns() // 1000,
                "monotonic_us": time.monotonic_ns() // 1000,
                "since_applied_us": time.monotonic_ns() // 1000 - applied,
                "remaining_generated_directories": remaining,
                "reset_epoch": epoch,
                "mode": mode,
            }
            if mode == "helper":
                props = dict(
                    x.split("=", 1)
                    for x in subprocess.check_output(
                        [
                            "systemctl",
                            "show",
                            "natsume-privileged-helper.service",
                            "-p",
                            "MainPID",
                            "-p",
                            "NRestarts",
                        ],
                        text=True,
                    ).splitlines()
                )
                pid = int(props["MainPID"])
                assert pid > 1
                r["pid"] = pid
                r["restart_count_before"] = int(props["NRestarts"])
                out.write(json.dumps(r) + "\n")
                os.kill(pid, signal.SIGKILL)
            else:
                out.write(json.dumps(r) + "\n")
                pathlib.Path("/run/natsume-bench-power-ready.json").write_text(
                    json.dumps(r) + "\n"
                )
            injected = True
        time.sleep(0.1)
PY
