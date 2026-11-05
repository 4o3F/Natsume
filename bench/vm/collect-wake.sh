#!/bin/sh
# Export fixed classifications and operation names; discard every error body in the guest.
set -eu
if [ "$#" -ne 4 ] && [ "$#" -ne 5 ]; then
    echo 'Usage: collect-wake.sh B WORK VM SINCE [UNTIL]' >&2
    exit 2
fi
exec python3 - "$@" <<'PY'
import pathlib, re, shlex, subprocess, sys

host, work, vm, since = sys.argv[1:5]
until = sys.argv[5:6]
if (
    not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.-]*@[A-Za-z0-9][A-Za-z0-9.-]*", host)
    or not pathlib.PurePosixPath(work).is_absolute()
    or not re.fullmatch("[A-Za-z0-9][A-Za-z0-9-]*", vm)
):
    raise SystemExit("invalid source")
code = r"""
import json,re,sys
for line in sys.stdin:
 try:r=json.loads(line)
 except ValueError:continue
 m=r.get('MESSAGE','')
 if not isinstance(m,str):continue
 out={'journal_at_us':int(r['__REALTIME_TIMESTAMP']),'journal_monotonic_us':int(r['__MONOTONIC_TIMESTAMP'])}
 match=re.match(r'^Removed session ([a-zA-Z0-9]+)\.$',m)
 if r.get('_SYSTEMD_UNIT')=='systemd-logind.service' and match:
  out.update(event='Removed session',session=match[1])
 elif (r.get('UNIT') in ['natsume-session-prepare@waiting.service','natsume-session-prepare@contest.service'] or re.search(r'natsume-session-prepare@(waiting|contest)\.service',m)):
  if m.startswith('Finished '):out['event']='preparation Finished'
  elif re.search(r': Deactivated successfully\.$',m):out['event']='preparation Deactivated'
  else:continue
  unit=r.get('UNIT','');out['role']='contest' if '@contest' in unit or '@contest' in m else 'waiting'
 else:continue
 print(json.dumps(out,separators=(',',':')))
"""

guest = (
    shlex.join(
        ["journalctl", "--since", since, "-o", "json", "--no-pager"]
        + (["--until", until[0]] if until else [])
    )
    + " | "
    + shlex.join(["python3", "-c", code])
)
launcher = r"""import pathlib,subprocess,tempfile,os,sys
work,vm,command=sys.argv[1:]
root=pathlib.Path(work)
for p in (root,*root.parents):
 s=p.lstat()
 if p.is_symlink() or s.st_uid!=0 or s.st_mode&0o022:raise SystemExit('untrusted work directory')
with tempfile.NamedTemporaryFile(mode='w',dir=work,prefix='wake-probe.',delete=False) as f:
 name=f.name;f.write('set -euo pipefail\n'+command+'\n')
try:sys.exit(subprocess.run([str(root/'qga.sh'),work,vm,name]).returncode)
finally:os.unlink(name)
"""
ssh = [
    "ssh",
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=yes",
    "-o",
    "UpdateHostKeys=no",
    "-o",
    "ForwardAgent=no",
    host,
]
sys.exit(
    subprocess.run(
        ssh + [shlex.join(["sudo", "-n", "python3", "-c", launcher, work, vm, guest])]
    ).returncode
)
PY
