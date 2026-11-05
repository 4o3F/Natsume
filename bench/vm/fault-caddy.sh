#!/bin/sh
# Guest-only reversible faults; runtime configuration contents are never exported.
set -eu
exec python3 - "$@" <<'PY'
import json, os, pathlib, pwd, signal, subprocess, sys, time, tomllib

mode, output = sys.argv[1:]
assert mode in ["crash", "admin"]
with open("/etc/natsume/config.toml", "rb") as f:
    hostname = tomllib.load(f)["site"]["gateway_hostname"]
sock = pathlib.Path("/run/natsume/caddy-admin.sock")
old = sock.stat()
owner = pwd.getpwuid(old.st_uid).pw_name


def unit(name):
    text = subprocess.check_output(
        [
            "systemctl",
            "show",
            name,
            "-p",
            "MainPID",
            "-p",
            "NRestarts",
            "-p",
            "ActiveState",
            "-p",
            "ActiveEnterTimestampMonotonic",
        ],
        text=True,
    )
    return {
        k: (int(v) if v.isdigit() else v)
        for line in text.splitlines()
        for k, _, v in [line.partition("=")]
        if v.isdigit()
        or v
        in ["active", "inactive", "activating", "deactivating", "failed", "reloading"]
    }


def http():
    r = subprocess.run(
        [
            "curl",
            "--noproxy",
            "*",
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            "--connect-timeout",
            ".2",
            "--max-time",
            ".3",
            "--cacert",
            "/etc/natsume/trust/local-origin-ca.crt",
            "--resolve",
            hostname + ":443:127.0.0.1",
            "https://" + hostname + "/",
        ],
        capture_output=True,
        text=True,
    )
    return {
        "http_status": int(r.stdout) if r.stdout.isdigit() else 0,
        "http_result": {
            0: "completed",
            7: "connection_failed",
            28: "timeout",
            35: "tls_connect_error",
            60: "tls_verify_failed",
        }.get(r.returncode, "other_error"),
    }


def sample():
    try:
        lines = (
            pathlib.Path("/var/lib/natsume-privileged/home-reset/progress")
            .read_text()
            .splitlines()
        )
        epoch = int(lines[0])
        phase = lines[1]
    except (OSError, ValueError):
        epoch, phase = 0, "unavailable"
    return {
        "caddy": unit("natsume-caddy.service"),
        "daemon": unit("natsume-device-daemon.service"),
        "reset_epoch": epoch,
        "phase": phase,
        "permit_present": pathlib.Path(
            "/run/natsume-privileged/contest-permit"
        ).exists(),
        "window_present": pathlib.Path(
            "/var/lib/natsume-privileged/home-reset/login-window"
        ).exists(),
        **http(),
    }


with open(output, "w", buffering=1) as f:

    def emit(kind, **fields):
        f.write(
            json.dumps(
                {
                    "kind": kind,
                    "at_us": time.time_ns() // 1000,
                    "monotonic_us": time.monotonic_ns() // 1000,
                    **fields,
                }
            )
            + "\n"
        )

    try:
        baseline = sample()
        emit("baseline", **baseline)
        if mode == "crash":
            pid = baseline["caddy"]["MainPID"]
            assert pid > 1
            emit("injection", mode=mode, pid=pid)
            os.kill(pid, signal.SIGKILL)
        else:
            emit(
                "injection",
                mode=mode,
                socket_inode=old.st_ino,
                socket_mode=old.st_mode & 0o777,
            )
            subprocess.run(
                ["runuser", "-u", owner, "--", "chmod", "000", str(sock)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=True,
            )
        previous = None
        end = time.monotonic() + 45
        while time.monotonic() < end:
            current = sample()
            if current != previous:
                emit("state", **current)
                previous = current
            time.sleep(0.1)
    finally:
        if mode == "admin" and sock.exists():
            now = sock.stat()
            if now.st_ino == old.st_ino and now.st_uid == old.st_uid:
                subprocess.run(
                    [
                        "runuser",
                        "-u",
                        owner,
                        "--",
                        "chmod",
                        oct(old.st_mode & 0o777)[2:],
                        str(sock),
                    ],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=True,
                )
            emit(
                "restored",
                socket_recreated=now.st_ino != old.st_ino,
                socket_mode=sock.stat().st_mode & 0o777,
            )
PY
