#!/bin/sh
# Read the guest journal once; filter it before compression and QGA transport.
set -eu
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
exec python3 - "$script_dir" "$@" <<'PY'
import ast
import base64
import gzip
import pathlib
import re
import shlex
import subprocess
import sys

root = pathlib.Path(sys.argv.pop(1))
if len(sys.argv) not in [5, 6]:
    raise SystemExit("Usage: collect-cycle.sh B WORK VM SINCE [UNTIL]")
host, work, vm, since = sys.argv[1:5]
until = sys.argv[5:]
if (
    not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.-]*@[A-Za-z0-9][A-Za-z0-9.-]*", host)
    or not pathlib.PurePosixPath(work).is_absolute()
    or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*", vm)
):
    raise SystemExit("invalid source")


def read_constant(filename, name):
    source = (root / filename).read_text().split("<<'PY'\n", 1)[1].rsplit("\nPY", 1)[0]
    for node in ast.parse(source).body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == name for target in node.targets
        ):
            return ast.literal_eval(node.value)
    raise SystemExit("collector constant missing")


filters = {
    label: read_constant(filename, name)
    for label, filename, name in [
        ("guest", "collect-events.sh", "filter_code"),
        ("retry", "collect-retry.sh", "code"),
        ("wake", "collect-wake.sh", "code"),
    ]
}
code = r'''
import base64
import contextlib
import gzip
import io
import json
import subprocess
import sys

raw = subprocess.check_output(
    ["journalctl", "-b", "--since", @SINCE@, "-o", "json", "--no-pager"] + @UNTIL@,
    text=True,
)
sys.argv = ["source-filter"]
records = []
for label, source in @FILTERS@.items():
    output = io.StringIO()
    sys.stdin = io.StringIO(raw)
    with contextlib.redirect_stdout(output):
        exec(compile(source, "source-filter", "exec"), {})
    for line in output.getvalue().splitlines():
        records.append(json.dumps({"source": label, "row": json.loads(line)}, separators=(",", ":")))
filtered = ("\n".join(records) + "\n").encode()
print(base64.b64encode(gzip.compress(filtered)).decode())
'''
code = code.replace("@SINCE@", repr(since))
code = code.replace("@UNTIL@", repr(["--until", *until] if until else []))
code = code.replace("@FILTERS@", repr(filters))
command = shlex.join(["python3", "-c", code])
launcher = read_constant("collect-retry.sh", "launcher")
ssh = [
    "ssh",
    "-o", "BatchMode=yes",
    "-o", "StrictHostKeyChecking=yes",
    "-o", "UpdateHostKeys=no",
    "-o", "ForwardAgent=no",
    host,
]
result = subprocess.run(
    ssh + [shlex.join(["sudo", "-n", "python3", "-c", launcher, work, vm, command])],
    capture_output=True,
)
if result.returncode:
    raise SystemExit("guest cycle collection failed; stderr withheld")
sys.stdout.buffer.write(gzip.decompress(base64.b64decode(result.stdout)))
PY
