#!/bin/sh
# Filter on A or inside the guest before any journal data crosses SSH/QGA.
set -eu
exec python3 - "$@" <<'PY'
import pathlib
import re
import shlex
import subprocess
import sys

usage = "Usage: collect-events.sh server HOST SINCE [DEVICE_ID] | guest HOST WORK NAME SINCE"
if len(sys.argv) < 4:
    raise SystemExit(usage)
source, host = sys.argv[1:3]
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
filter_code = r"""
import json,re,sys
EVENTS={'target dispatched','actual received','target received','reconcile completed','session step completed','actual sent','session switch completed','home reset completed','home reset background completed','database transaction wait exceeded threshold','home recovery','frame presented','startup milestone','home step completed','caddy load completed','binding eligibility changed'}
FIELD=re.compile(r'\b(db\.(?:operation|result|permit_wait_us|pool_wait_us|write_lock_wait_us|total_wait_us)|[a-z][a-z0-9_]*)(?:=)("[^"\n]*"|[^\s,}]+)')
for line in sys.stdin:
 try: row=json.loads(line)
 except ValueError: continue
 message=row.get('MESSAGE','')
 if not isinstance(message,str):continue
 if len(sys.argv)>1 and sys.argv[1] not in message:continue
 event=next((e for e in EVENTS if re.match(r'^\s*INFO\s+'+re.escape(e)+r'(?:\s|$)',message)),None)
 if not event:continue
 out={'event':event,'journal_at_us':int(row['__REALTIME_TIMESTAMP']),'journal_monotonic_us':int(row['__MONOTONIC_TIMESTAMP'])}
 for key,value in FIELD.findall(message):
  value=value.strip('"')
  if key=='target_digest' and re.fullmatch('[0-9a-f]{16}',value):out[key]=value
  elif (key in {'plan','completed_reset_epoch','completed_terminate_epoch'} or key.endswith(('_us','_ms'))) and value.isdigit():out[key]=int(value)
  elif key in {'outcome','result'} and value in {'ok','error','timeout','rpc_error','unavailable','rejected','retry','completed','cancelled'}:out[key]=value
  elif key in {'waiting_ready','contest_ready'} and value in {'true','false'}:out[key]=value=='true'
  elif key=='stage' and value in {'starting','system_bus','helper','identity_preflight','hardware_identity','local_gateway','control_identity','reconciliation','control_configuration','connecting','handshake','pending_review','authority_installation','active'}:out[key]=value
  elif key=='status' and value in {'starting','retrying','waiting_for_review','denied','active','stopped'}:out[key]=value
  elif key=='operation' and value in {'prepare','activate','terminate','prepare_boot','recover_waiting','resume_waiting','cleanup','remove_generations','prepare_generation','prepare_request','apply_request','verify_request','recover_request','recover_local','apply','verify','close_admission'}:out[key]=value
  elif key in {'foreground_target','role','session_foreground'} and value in {'waiting','contest','none','unknown','greeter','other'}:out[key]=value
  elif key=='mode' and value in {'ready','blocked'}:out[key]=value
  elif key=='action' and value in {'resume','baseline'}:out[key]=value
  elif key=='reset_epoch' and value.isdigit():out[key]=int(value)
  elif key=='home_state' and value in {'steady','resetting','recovery_required'}:out[key]=value
  elif key=='session_state' and value in {'running','none','starting','terminating','ambiguous','error'}:out[key]=value
  elif key=='phase' and value in {'remove_generations','verify','publish_permit'}:out[key]=value
  elif key=='error_category' and value in {'rejected','unavailable','task_stopped','incomplete'}:out[key]=value
  elif key=='registration_stage' and value in {'observer','greeter_lookup','greeter_activation','probe','proof','complete'}:out[key]=value
  elif key=='registration_kind' and value in {'not_sampled','greeter_replaced','probe_error','probe_unavailable','proof_missing','proof_mismatch','proof_cache_mismatch','gdm_unavailable','proof_ambiguous','registered'}:out[key]=value
  elif key=='code' and value=='db_transaction_wait':out[key]=value
  elif key=='db.operation' and value in {'read','write'}:out[key]=value
  elif key=='db.result' and value in {'ok','error'}:out[key]=value
 print(json.dumps(out,separators=(',',':')))
"""
if source == "server" and len(sys.argv) in [4, 5]:
    since = sys.argv[3]
    identifier = sys.argv[4:5]
    if identifier and not re.fullmatch("[0-9a-f-]{36}", identifier[0]):
        raise SystemExit("invalid device ID")
    command = (
        shlex.join(
            [
                "sudo",
                "-n",
                "journalctl",
                "-u",
                "natsume-bench",
                "--since",
                since,
                "-o",
                "json",
                "--no-pager",
            ]
        )
        + " | "
        + shlex.join(["python3", "-c", filter_code, *identifier])
    )
    result = subprocess.run(ssh + ["bash -o pipefail -c " + shlex.quote(command)])
elif source == "guest" and len(sys.argv) == 6:
    work, name, since = sys.argv[3:6]
    if not pathlib.PurePosixPath(work).is_absolute() or not re.fullmatch(
        "[A-Za-z0-9][A-Za-z0-9-]*", name
    ):
        raise SystemExit("invalid VM directory")
    guest_command = (
        shlex.join(["journalctl", "-b", "--since", since, "-o", "json", "--no-pager"])
        + " | "
        + shlex.join(["python3", "-c", filter_code])
    )
    launcher = r"""import pathlib,subprocess,tempfile,sys,os
work,name,command=sys.argv[1:4]
root=pathlib.Path(work)
for directory in (root,*root.parents):
 st=directory.lstat()
 if directory.is_symlink() or st.st_uid!=0 or st.st_mode&0o022:raise SystemExit('untrusted VM directory')
p=None
try:
 with tempfile.NamedTemporaryFile(mode='w',dir=work,prefix='event-probe.',delete=False) as f:
  p=f.name;f.write('set -euo pipefail\n'+command+'\n')
 result=subprocess.run([str(pathlib.Path(work)/'qga.sh'),work,name,p])
 sys.exit(result.returncode)
finally:
 if p:os.unlink(p)
"""
    result = subprocess.run(
        ssh
        + [
            shlex.join(
                ["sudo", "-n", "python3", "-c", launcher, work, name, guest_command]
            )
        ]
    )
else:
    raise SystemExit(usage)
sys.exit(result.returncode)
PY
