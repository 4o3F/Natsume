#!/bin/sh
# Guest injector: pause only the captured GDM/greeter identity; always resume it.
set -eu
exec python3 - "$@" <<'PY'
import json, os, pathlib, pwd, signal, sys, time

output = sys.argv[1]
assert output.startswith("/run/")
record = pathlib.Path("/run/natsume-privileged/session-preparation.json")
stopped = {}
injected = None
owner = None
phase = None
end = time.monotonic() + 150
previous = None
uid = pwd.getpwnam("gdm").pw_uid
signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))


def identity(pid):
    try:
        p = pathlib.Path("/proc", str(pid))
        data = (p / "stat").read_text()
        fields = data[data.rfind(")") + 2 :].split()
        return int(fields[19]), fields[0]
    except (OSError, ValueError, IndexError):
        return None


with open(output, "w", buffering=1) as out:

    def emit(kind, **fields):
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

    def pause(pid, role):
        ident = identity(pid)
        if ident and pid > 1 and pid not in stopped:
            stopped[pid] = (ident[0], role)
            os.kill(pid, signal.SIGSTOP)
            emit("stopped", pid=pid, start_ticks=ident[0], role=role, phase=phase)

    def resume():
        for pid, (start, role) in stopped.items():
            current = identity(pid)
            if current and current[0] == start:
                try:
                    os.kill(pid, signal.SIGCONT)
                    emit("resumed", pid=pid, start_ticks=start, role=role)
                except ProcessLookupError:
                    emit("terminated", pid=pid, start_ticks=start, role=role)
            else:
                emit("terminated", pid=pid, start_ticks=start, role=role)

    try:
        while time.monotonic() < end:
            try:
                d = json.loads(record.read_text())
                phase = d["phase"]
                pid = int(d["pid"])
                if phase not in [
                    "greeter",
                    "registration",
                    "authentication",
                    "desktop",
                    "ready",
                    "failed",
                ]:
                    raise ValueError()
                current = {
                    "phase": phase,
                    "preparation_pid": pid,
                    "boot_prepared": pathlib.Path(
                        "/run/natsume-privileged/boot-prepared"
                    ).exists(),
                }
                if current != previous:
                    emit("state", **current)
                    previous = current
                if d.get("role") == "contest" and (
                    (injected is None and phase == "registration")
                    or (injected is not None and time.monotonic() - injected < 36)
                ):
                    for p in pathlib.Path("/proc").iterdir():
                        if p.name.isdigit():
                            try:
                                if (
                                    p.stat().st_uid == uid
                                    and (p / "comm").read_text().strip()
                                    == "gnome-shell"
                                ):
                                    pause(int(p.name), "greeter")
                                    if injected is None:
                                        owner = pid
                                        injected = time.monotonic()
                                        emit(
                                            "injection",
                                            preparation_pid=owner,
                                            phase=phase,
                                        )
                            except OSError:
                                pass
                if injected is not None and (
                    phase == "failed" or time.monotonic() - injected >= 41
                ):
                    emit("release", phase=phase)
                    break
            except (OSError, ValueError, KeyError):
                pass
            time.sleep(0.02)
        assert injected is not None, "no eligible preparation observed"
    finally:
        resume()
    for pid, (start, role) in stopped.items():
        cur = identity(pid)
        emit(
            "checked",
            pid=pid,
            start_ticks=start,
            role=role,
            present=bool(cur and cur[0] == start),
            stopped=bool(cur and cur[0] == start and cur[1] in ["T", "t"]),
        )
PY
