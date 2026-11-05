#!/bin/sh
# Guest-only read-only admission probes; never authenticate or create a PAM session.
set -eu
exec python3 - "$@" <<'PY'
import fcntl, hashlib, json, os, pathlib, subprocess, sys, time

mode = sys.argv[1]
assert mode in ["snapshot", "watch"]
root = pathlib.Path("/var/lib/natsume-privileged/home-reset")
runtime = pathlib.Path("/run/natsume-privileged")


def sample():
    ep, phase = (root / "progress").read_text().splitlines()
    assert phase in ["draining", "prepared", "applied", "verified", "recovery_required"]
    permit = runtime / "contest-permit"
    boot = pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    present = permit.exists()
    matches = present and permit.read_text() == boot
    window = (root / "login-window").exists()
    lock = False
    fd = os.open(runtime / "contest-admission.lock", os.O_RDONLY | os.O_NOFOLLOW)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
            lock = True
            fcntl.flock(fd, fcntl.LOCK_UN)
        except BlockingIOError:
            pass
    finally:
        os.close(fd)
    return {
        "reset_epoch": int(ep),
        "phase": phase,
        "permit_present": present,
        "permit_boot_matches": matches,
        "window_present": window,
        "shared_lock_available": lock,
        "generations_count": len(list((root / "generations").iterdir())),
        "gate_result": "GATE_ALLOWED"
        if matches and not window and lock and phase == "verified"
        else "GATE_BLOCKED",
    }


def emit(out, kind, **fields):
    out.write(
        json.dumps(
            dict(
                kind=kind,
                at_us=time.time_ns() // 1000,
                monotonic_us=time.monotonic_ns() // 1000,
                **fields,
            )
        )
        + "\n"
    )
    out.flush()


if mode == "snapshot":
    r = sample()
    b = subprocess.run(
        [
            "runuser",
            "-u",
            "natsume",
            "--",
            "busctl",
            "--system",
            "--timeout=3",
            "call",
            "org.natsume.Privileged1",
            "/org/natsume/Privileged1",
            "org.natsume.Privileged1",
            "IsHomeReady",
        ],
        capture_output=True,
        text=True,
    )
    r["helper_home_ready"] = b.returncode == 0 and b.stdout.strip() == "b true"
    hashes = {}
    for label, p in [
        ("contest", "/etc/pam.d/gdm-contest"),
        ("admission", "/etc/pam.d/natsume-contest-admission"),
    ]:
        hashes[label] = hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
    text = pathlib.Path("/etc/pam.d/natsume-contest-admission").read_text()
    r["pam_hooks"] = {
        k: any(
            line.split()
            == [
                k,
                "requisite",
                "pam_exec.so",
                "quiet",
                "/usr/lib/natsume/natsume-privileged-helper",
                "pam-gate",
            ]
            for line in text.splitlines()
        )
        for k in ["auth", "account", "session"]
    }
    r["hashes"] = hashes
    emit(sys.stdout, "snapshot", **r)
else:
    minimum = int(sys.argv[2])
    output = sys.argv[3]
    previous = None
    attempted = False
    end = time.monotonic() + 180
    with open(output, "w", buffering=1) as out:
        while time.monotonic() < end:
            r = sample()
            if r != previous:
                emit(out, "state", **r)
                previous = r
            if (
                r["reset_epoch"] > minimum
                and r["phase"] == "applied"
                and r["window_present"]
                and r["generations_count"] == 2
                and not attempted
            ):
                emit(out, "admission_attempt", **r)
                attempted = True
            if attempted and r["phase"] == "verified" and not r["window_present"]:
                emit(out, "admission_attempt", **r)
                break
            time.sleep(0.1)
        assert attempted, "no reset window observed"
PY
