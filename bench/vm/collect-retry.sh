#!/bin/sh
# Export fixed classifications and operation names; discard every error body in the guest.
set -eu
if [ "$#" -ne 4 ] && [ "$#" -ne 5 ]; then
    echo 'Usage: collect-retry.sh B WORK VM SINCE [UNTIL]' >&2
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
PREFIXES=['Immediate safety exit; diagnostic file publication skipped','Device active snapshot processing failed','Local resource operation did not complete','Graphical preparation incomplete; resample before retry','Graphical preparation phase changed','Local maintenance will retry','Discarding unverified startup observations','GDM registration observation available','GDM acknowledged greeter startup','GDM registration observation will reconnect','Could not confirm waiting after preparation failure','Owned waiting recovery remains incomplete','Waiting recovery could not complete']
EVENTS=['session switch completed','session step completed','reconcile completed','home reset completed','home step completed']
OPERATIONS={'activate','prepare','terminate','prepare_boot','recover_waiting','resume_waiting','prepare_reset','verify_reset','apply_reset','close_admission','cleanup','remove_generations','prepare_generation','prepare_request','apply_request','verify_request','recover_request','recover_local','apply','verify'}
FIELD=re.compile(r'\b([a-z][a-z0-9_]*)=("[^"\n]*"|[^\s,}]+)')
for line in sys.stdin:
 try:r=json.loads(line)
 except ValueError:continue
 m=r.get('MESSAGE','')
 if not isinstance(m,str):continue
 event=next((p for p in PREFIXES+EVENTS if re.match(r'^\s*(INFO|WARN|ERROR)\s+'+re.escape(p)+r'(?:\s|$)',m)),None)
 if not event:continue
 o={'event':event,'journal_at_us':int(r['__REALTIME_TIMESTAMP']),'journal_monotonic_us':int(r['__MONOTONIC_TIMESTAMP'])}
 for k,v in FIELD.findall(m):
  v=v.strip('"')
  if (k in ['plan','reset_epoch'] or k.endswith(('_us','_ms'))) and v.isdigit():o[k]=int(v)
  elif k=='operation' and v in OPERATIONS:o[k]=v
  elif k in ['result','outcome'] and v in ['ok','error','timeout','retry','completed','cancelled','unavailable','rejected','rpc_error']:o[k]=v
  elif k=='code' and v in ['gdm_registration_cache_invalid','gdm_registration_observer_ready','greeter_registered','gdm_registration_observer_unavailable']:o['kind']=v
  elif k=='code' and v=='waiting_restore_unconfirmed':o['kind']='waiting_restore_unconfirmed'
  elif k=='code' and v=='LOCAL_DEACTIVATION_FAILED':o['kind']='local_deactivation_failed'
  elif k=='phase' and v in ['Greeter','Registration','Authentication','Desktop','Ready','Failed']:o[k]=v
  elif k.endswith('_retry') and k in ['gateway_retry','runtime_retry','session_retry','home_retry','power_retry','caddy_retry'] and v in ['true','false']:o[k]=v=='true'
 if event in PREFIXES and 'error=' in m:
  tail=m.split('error=',1)[1].lstrip('"')
  # Match only reviewed constant prefixes; never return the diagnostic body.
  for prefix,kind in [('org.natsume.Privileged1.Error.Unavailable: another graphical mutation is in progress','graphical_mutation_busy'),('org.natsume.Privileged1.Error.Unavailable: logind session termination is incomplete','logind_termination_incomplete'),('org.natsume.Privileged1.Error.Unavailable: graphical desktop is not ready','desktop_not_ready'),('org.natsume.Privileged1.Error.Unavailable: another session preparation is in progress','preparation_in_progress'),('org.natsume.Privileged1.Error.Unavailable: preparation_timeout:','preparation_timeout'),('org.natsume.Privileged1.Error.Rejected: logind operation was rejected','logind_operation_rejected')]:
   if tail.startswith(prefix):o['kind']=kind;break
  if tail.startswith('Caddy state could not be applied or sampled'):o['classification']='Caddy'
  if tail.startswith('local capability service is unavailable'):o['classification']='LocalControl'
  if tail.startswith('I/O error:'):o['classification']='InputOutput'
  if tail.startswith('org.freedesktop.zbus.Error:'):o['classification']='ZBus'
  if tail.startswith('org.freedesktop.zbus.Error: i/o error'):o['classification']='InputOutput'
  for name in ['Unavailable','Rejected','NoReply','Timeout','Disconnected','ServiceUnknown','NameHasNoOwner']:
   if re.search(r'(?<![a-zA-Z])'+name+r'(?=[:(\s".]|$)',tail):o['classification']=name;break
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
