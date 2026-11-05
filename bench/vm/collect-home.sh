#!/bin/sh
# Filter Home timings and numeric cleanup counters inside the guest.
set -eu
if [ "$#" -ne 4 ] && [ "$#" -ne 5 ]; then
    echo 'Usage: collect-home.sh B WORK VM SINCE [UNTIL]' >&2
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
EVENTS={'home reset completed','home step completed','reconcile completed','target received','actual sent'}
OPS={'cleanup','remove_generations','prepare_generation','prepare_request','apply_request','verify_request','recover_request','prepare','apply','recover','verify','recover_local'}
FIELD=re.compile(r'\b([a-z][a-z0-9_]*)=("[^"\n]*"|[^\s,}]+)')
for line in sys.stdin:
 try:r=json.loads(line)
 except ValueError:continue
 m=r.get('MESSAGE','')
 if not isinstance(m,str):continue
 event=next((p for p in EVENTS if re.match(r'^\s*INFO\s+'+re.escape(p)+r'(?:\s|$)',m)),None)
 if not event:continue
 o={'event':event,'journal_at_us':int(r['__REALTIME_TIMESTAMP']),'journal_monotonic_us':int(r['__MONOTONIC_TIMESTAMP'])}
 for k,v in FIELD.findall(m):
  v=v.strip('"')
  if (k in ['plan','reset_epoch','completed_reset_epoch','removed_entries','removed_bytes'] or k.endswith(('_us','_ms'))) and v.isdigit():o[k]=int(v)
  elif k=='operation' and v in OPS:o[k]=v
  elif k in ['result','outcome'] and v in ['ok','error','timeout','retry','completed','cancelled','unavailable','rejected','rpc_error']:o[k]=v
  elif k=='target_digest' and re.fullmatch('[0-9a-f]{16}',v):o[k]=v
  elif k in ['home_state','session_state','session_foreground'] and v in ['steady','resetting','recovery_required','running','starting','terminating','error','unknown','none','waiting','contest','greeter','other']:o[k]=v
  elif k in ['waiting_ready','contest_ready'] and v in ['true','false']:o[k]=v=='true'
  elif k=='deadline_hit' and v in ['true','false']:o[k]=v=='true'
 print(json.dumps(o,separators=(',',':')))

"""
guest = (
    shlex.join(
        ["journalctl", "-b", "--since", since, "-o", "json", "--no-pager"]
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
with tempfile.NamedTemporaryFile(mode='w',dir=work,prefix='retry-probe.',delete=False) as f:
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
