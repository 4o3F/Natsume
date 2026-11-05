#!/bin/sh
# Execute a reviewed probe from a root-owned VM host; never pass secret-printing probes.
set -eu
umask 077
if [ "$#" -ne 3 ] && [ "$#" -ne 5 ]; then
    echo 'Usage: qga.sh WORK NAME SCRIPT [PUBLIC_SOURCE GUEST_DESTINATION]' >&2
    exit 2
fi
test "$(id -u)" = 0
exec python3 - "$@" <<'PY'
import base64
import json
import os
import pathlib
import re
import socket
import sys
import time

work, name, script = sys.argv[1:4]
root = pathlib.Path(work)
if not root.is_absolute() or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*", name):
    raise SystemExit("invalid VM directory")
for p in (root, *root.parents, root / name):
    st = p.lstat()
    if p.is_symlink() or st.st_uid != 0 or st.st_mode & 0o022:
        raise SystemExit("untrusted VM directory")
s = socket.socket(socket.AF_UNIX)
s.settimeout(30)
s.connect(str(root / name / "guest-agent.sock"))
f = s.makefile("rwb", buffering=0)


def call(command, arguments):
    ident = time.time_ns()
    f.write(
        json.dumps({"execute": command, "arguments": arguments, "id": ident}).encode()
        + b"\n"
    )
    while True:
        line = f.readline()
        if not line:
            raise SystemExit("QGA connection closed")
        response = json.loads(line.lstrip(b"\xff"))
        if response.get("id") == ident:
            if "error" in response:
                raise SystemExit("QGA operation rejected; response withheld")
            return response["return"]


f.write(b"\xff")
call("guest-sync-delimited", {"id": time.time_ns()})
if len(sys.argv) == 6:
    source, destination = sys.argv[4:6]
    if not destination.startswith("/run/") or ".." in destination.split("/"):
        raise SystemExit("public transfers must target /run")
    handle = call("guest-file-open", {"path": destination, "mode": "wb"})
    with open(source, "rb") as stream:
        while chunk := stream.read(16384):
            result = call(
                "guest-file-write",
                {"handle": handle, "buf-b64": base64.b64encode(chunk).decode()},
            )
            if result["count"] != len(chunk):
                raise SystemExit("incomplete public-file transfer")
    call("guest-file-close", {"handle": handle})
limit = int(os.environ.get("QGA_TIMEOUT_SECONDS", "180"))
if not 1 <= limit <= 600:
    raise SystemExit("invalid QGA timeout")
pid = call(
    "guest-exec",
    {
        "path": "/bin/bash",
        "arg": ["-c", pathlib.Path(script).read_text()],
        "capture-output": True,
    },
)["pid"]
deadline = time.monotonic() + limit
while time.monotonic() < deadline:
    result = call("guest-exec-status", {"pid": pid})
    if result.get("exited"):
        if result.get("out-truncated") or result.get("err-truncated"):
            raise SystemExit("QGA output truncated; shorten the collection window")
        output = base64.b64decode(result.get("out-data", ""))
        sys.stdout.buffer.write(output)
        if result.get("exitcode", 0):
            raise SystemExit("guest operation failed; stderr withheld")
        break
    time.sleep(0.05)
else:
    raise SystemExit("guest operation timed out")
s.close()
PY
