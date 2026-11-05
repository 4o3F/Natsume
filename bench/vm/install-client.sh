#!/bin/sh
# Run as root on B while the paired bench Server is stopped.
set -eu
umask 077
if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
    echo 'Usage: install-client.sh WORK NAME CLIENT_DEB [DEPENDENCY_DIRECTORY]' >&2
    exit 2
fi
test "$(id -u)" = 0
exec python3 - "$@" <<'PY'
import base64
import hashlib
import json
import pathlib
import re
import shlex
import socket
import sys
import time

work, name, package = sys.argv[1:4]
if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*", name):
    raise SystemExit("invalid VM name")
root = pathlib.Path(work)
if not root.is_absolute():
    raise SystemExit("work directory must be absolute")
for directory in (root, *root.parents, root / name):
    mode = directory.lstat()
    if directory.is_symlink() or mode.st_uid != 0 or mode.st_mode & 0o022:
        raise SystemExit("untrusted work directory")
packages = sorted(pathlib.Path(sys.argv[4]).glob("*.deb")) if len(sys.argv) == 5 else []
packages.append(pathlib.Path(package))
if any(not p.is_absolute() or not p.is_file() for p in packages):
    raise SystemExit("packages must be existing absolute paths")

connection = socket.socket(socket.AF_UNIX)
connection.settimeout(30)
connection.connect(str(root / name / "guest-agent.sock"))
channel = connection.makefile("rwb", buffering=0)

def call(command, arguments):
    request_id = time.time_ns()
    channel.write(json.dumps({"execute": command, "arguments": arguments, "id": request_id}).encode() + b"\n")
    while True:
        response = json.loads(channel.readline().lstrip(b"\xff"))
        if response.get("id") == request_id:
            if "error" in response:
                raise SystemExit("QGA request failed; response withheld")
            return response["return"]

def execute(script):
    pid = call("guest-exec", {"path": "/bin/bash", "arg": ["-c", script], "capture-output": True})["pid"]
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        result = call("guest-exec-status", {"pid": pid})
        if result.get("exited"):
            if result.get("exitcode", 0):
                raise SystemExit("guest operation failed; output withheld")
            return base64.b64decode(result.get("out-data", "")).decode()
        time.sleep(0.2)
    raise SystemExit("guest operation timed out")

channel.write(b"\xff")
call("guest-sync-delimited", {"id": time.time_ns()})
execute("set -e; systemctl stop natsume-device-daemon natsume-privileged-helper natsume-caddy; install -d -m 700 /run/natsume-step33")
guest_packages = []
for index, source in enumerate(packages):
    destination = f"/run/natsume-step33/package-{index}.deb"
    handle = call("guest-file-open", {"path": destination, "mode": "wb"})
    digest = hashlib.sha256()
    try:
        with source.open("rb") as stream:
            while chunk := stream.read(16384):
                digest.update(chunk)
                result = call("guest-file-write", {"handle": handle, "buf-b64": base64.b64encode(chunk).decode()})
                if result["count"] != len(chunk):
                    raise SystemExit("incomplete package transfer")
    finally:
        call("guest-file-close", {"handle": handle})
    execute(f"test \"$(sha256sum {destination} | cut -d' ' -f1)\" = {digest.hexdigest()}")
    guest_packages.append(destination)

paths = shlex.join(guest_packages)
output = execute(f"""set -e
umask 077
dpkg -i {paths} > /run/natsume-step33/install.log 2>&1
systemctl daemon-reload
systemctl enable --now systemd-timesyncd >/dev/null 2>&1
systemctl start natsume-privileged-helper natsume-caddy natsume-device-daemon
systemctl restart gdm.service
dpkg-query -W -f='client_version=${{Version}}\n' natsume-client
timedatectl show -p NTP -p NTPSynchronized
"""
)
for line in output.splitlines():
    if re.fullmatch(r"client_version=[0-9A-Za-z.+~:-]+|NTP=(yes|no)|NTPSynchronized=(yes|no)", line):
        print(line)
connection.close()
PY
