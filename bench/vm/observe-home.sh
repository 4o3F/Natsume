#!/bin/sh
# Observe fixed Home progress fields and admission marker existence without reading identities.
set -eu
exec python3 - "$@" <<'PY'
import json
import pathlib
import sys
import time

seconds = int(sys.argv[1]) if len(sys.argv) == 2 else 1200
assert 1 <= seconds <= 1800
state = pathlib.Path("/var/lib/natsume-privileged/home-reset")
previous = None
end = time.monotonic() + seconds
while time.monotonic() < end:
    try:
        lines = (state / "progress").read_text().splitlines()
        epoch = int(lines[0])
        phase = lines[1]
        assert phase in [
            "draining",
            "prepared",
            "applied",
            "verified",
            "recovery_required",
        ]
    except (OSError, ValueError, IndexError, AssertionError):
        epoch, phase = 0, "unavailable"
    current = {
        "reset_epoch": epoch,
        "phase": phase,
        "permit_present": pathlib.Path(
            "/run/natsume-privileged/contest-permit"
        ).exists(),
        "window_present": (state / "login-window").exists(),
        "generations_count": len(list((state / "generations").iterdir())),
    }
    if current != previous:
        print(
            json.dumps(
                {
                    "journal_at_us": time.time_ns() // 1000,
                    "monotonic_us": time.monotonic_ns() // 1000,
                    **current,
                }
            ),
            flush=True,
        )
        previous = current
    time.sleep(0.1)
PY
