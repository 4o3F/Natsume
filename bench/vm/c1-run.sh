#!/bin/sh
# Drive one workstation at a time; credentials are read only by the B-side process.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
exec python3 - "$script_dir" "$@" <<'PY'
from bench_env import configured

CFG = configured()
import datetime
import os
import json
import pathlib
import re
import shlex
import subprocess
import sys
import time

script_dir = pathlib.Path(sys.argv.pop(1))
if len(sys.argv) not in [4, 5, 6]:
    raise SystemExit("Usage: c1-run.sh 1|2 COUNT OUTPUT [COOLDOWN_SECONDS [KINDS]]")
slot, count, output = sys.argv[1:4]
if slot not in ["1", "2"]:
    raise SystemExit("VM slot must be 1 or 2")
server, host, work = (CFG["A"], CFG["B"], CFG["VM_ROOT"])
vm, device, seat = (CFG["VM" + slot], CFG["DEVICE" + slot], CFG["SEAT" + slot])
cooldown = float(sys.argv[4]) if len(sys.argv) >= 5 else 3.0
kinds = sys.argv[5] if len(sys.argv) == 6 else "contest,waiting,terminate,home_reset,binding"
if not kinds or any(k not in {"contest", "waiting", "terminate", "home_reset", "binding"} for k in kinds.split(",")):
    raise SystemExit("invalid operation kinds")
if not all(
    (
        re.fullmatch("[A-Za-z_][A-Za-z0-9_.-]*@[A-Za-z0-9][A-Za-z0-9.-]*", h)
        for h in [server, host]
    )
):
    raise SystemExit("invalid SSH host")
if not pathlib.PurePosixPath(work).is_absolute() or not re.fullmatch(
    "[A-Za-z0-9][A-Za-z0-9-]*", vm
):
    raise SystemExit("invalid VM directory")
if not re.fullmatch("[0-9a-f-]{36}", device) or not re.fullmatch("S[0-9]{5}", seat):
    raise SystemExit("invalid device or seat")
count = int(count)
if not 1 <= count <= 100 or not 0 <= cooldown <= 60:
    raise SystemExit("invalid count or cooldown")
destination = pathlib.Path(output)
destination.mkdir(parents=True, exist_ok=False)
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
]
collector = script_dir / "collect-events.sh"
remote = r"""
import datetime,http.cookiejar,json,pathlib,socket,ssl,sys,time,urllib.error,urllib.request,uuid
work,vm,device,seat,iteration,cooldown,origin,kinds=sys.argv[1:]
kinds=set(kinds.split(","))
iteration=int(iteration);cooldown=float(cooldown)
base=pathlib.Path('@BENCH_AUTH_DIR@')
opener=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=str(base/'control-ca.crt'))))
def api(method,path,body=None):
 req=urllib.request.Request(origin+path,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'},method=method)
 try:
  with opener.open(req,timeout=15) as r:
   data=r.read();return json.loads(data) if data else None
 except urllib.error.HTTPError as e:raise SystemExit('API HTTP status '+str(e.code)) from None
 except Exception:raise SystemExit('API transport failed; details withheld') from None
api('POST','/session',{'login_name':'@BENCH_ADMIN_LOGIN@','password':(base/'operator-password').read_text().rstrip('\n')})
def wait(bound=True,mode='waiting',after_ms=0):
 deadline=time.monotonic()+100
 while time.monotonic()<deadline:
  d=api('GET','/devices/'+device+'/convergence');s=d['session_control'];h=d['home'];b=d['binding'];g=d['gateway'];a=s.get('actual') or {};ba=b.get('actual') or {}
  ready=d['connection_state']=='active' and s['status']=='converged' and h['status']=='converged' and h.get('actual',{}).get('state')=='steady' and a.get('session_state')=='running' and a.get('foreground')==mode and a.get('contest_ready') and a.get('waiting_ready') and b['status']=='converged' and (d.get('received_at_unix_ms') or 0)>=after_ms
  if bound:ready=ready and ba.get('assignment_state')=='applied' and ba.get('credential_state')=='applied' and g['status']=='converged' and g.get('actual',{}).get('state')=='ready'
  else:ready=ready and ba.get('assignment_state')=='absent' and ba.get('credential_state')=='absent' and g.get('actual',{}).get('state')=='blocked'
  if ready:return d
  time.sleep(.1)
 raise SystemExit('convergence timeout; diagnostic text withheld')
def emit(kind,operation,before,submitted,final,phase='target',first=None):
 print(json.dumps({'vm':vm,'device_id':device,'sample':iteration,'kind':kind,'phase':phase,'operation_id':operation,'server_operation_id':operation if phase=='target' else None,'before_us':before,'submitted_us':submitted,'received_at_unix_ms':final['received_at_unix_ms'],'outcome':'converged','roundtrip_before_us':first}),flush=True)
def enter_seat():
 s=socket.socket(socket.AF_UNIX);s.settimeout(1);s.connect(str(pathlib.Path(work)/vm/'monitor.sock'))
 s.recv(4096)
 for key in ['ctrl-a','shift-s',*list(seat[1:]),'ret']:
  s.sendall(('sendkey '+key+'\n').encode());time.sleep(.1)
  try:s.recv(4096)
  except TimeoutError:pass
 s.close()
wait()
for kind,action,mode in [('contest',{'kind':'set_foreground','foreground_target':'contest'},'contest'),('waiting',{'kind':'set_foreground','foreground_target':'waiting'},'waiting'),('terminate',{'kind':'terminate_session'},'waiting'),('home_reset',{'kind':'reset_home'},'waiting')]:
 if kind not in kinds:continue
 operation=str(uuid.uuid4());before=time.time_ns()//1000
 api('POST','/target-submissions',{'operation_id':operation,'scope':{'kind':'devices','device_ids':[device]},'action':action})
 submitted=time.time_ns()//1000;final=wait(mode=mode,after_ms=before//1000-50)
 emit(kind,operation,before,submitted,final);time.sleep(cooldown)
if 'binding' not in kinds:raise SystemExit(0)
operation=str(uuid.uuid4());before=time.time_ns()//1000
api('DELETE','/devices/'+device+'/binding');submitted=time.time_ns()//1000
final=wait(False,after_ms=before//1000-50)
emit('binding',operation,before,submitted,final,'unbind')
rebind=time.time_ns()//1000;enter_seat();submitted=time.time_ns()//1000
final=wait(after_ms=rebind//1000-50)
emit('binding',operation,rebind,submitted,final,'rebind',before)
time.sleep(cooldown)
"""
for sample in range(1, count + 1):
    if time.time() > float(os.environ.get("NATSUME_C1_DEADLINE", "inf")):
        raise SystemExit("sampling deadline reached between complete cycles")
    since = datetime.datetime.fromtimestamp(
        time.time() - 2, datetime.timezone.utc
    ).strftime("%Y-%m-%d %H:%M:%S UTC")
    command = shlex.join(
        [
            "sudo",
            "-n",
            "python3",
            "-",
            work,
            vm,
            device,
            seat,
            str(sample),
            str(cooldown),
            CFG["SERVER_URL"] + "/api/v2",
            kinds,
        ]
    )
    result = subprocess.run(
        ssh + [host, command], input=remote, text=True, capture_output=True
    )
    operations = destination / f"{sample:03d}-operations.jsonl"
    operations.write_text(result.stdout)
    if result.returncode:
        raise SystemExit(f"{vm} cycle {sample} failed; remote stderr withheld")
    with (destination / f"{sample:03d}-server.jsonl").open("w") as stream:
        status = subprocess.run(
            ["sh", str(collector), "server", server, since, device],
            stdout=stream, stderr=subprocess.PIPE,
        )
    if status.returncode:
        raise SystemExit("server collection failed; stderr withheld")
    bundle = subprocess.run(
        ["sh", str(script_dir / "collect-cycle.sh"), host, work, vm, since],
        capture_output=True, text=True,
    )
    if bundle.returncode == 0:
        grouped = {label: [] for label in ["guest", "retry", "wake"]}
        for line in bundle.stdout.splitlines():
            item = json.loads(line)
            grouped[item["source"]].append(item["row"])
        for label, records in grouped.items():
            (destination / f"{sample:03d}-{label}.jsonl").write_text(
                "".join(json.dumps(row) + "\n" for row in records)
            )
    else:
        # The individual collectors remain a fallback for QGA transport failures.
        for label, script, args in [
            ("guest", "collect-events.sh", ["guest", host, work, vm, since]),
            ("retry", "collect-retry.sh", [host, work, vm, since]),
            ("wake", "collect-wake.sh", [host, work, vm, since]),
        ]:
            with (destination / f"{sample:03d}-{label}.jsonl").open("w") as stream:
                status = subprocess.run(
                    ["sh", str(script_dir / script), *args],
                    stdout=stream, stderr=subprocess.PIPE,
                )
            if status.returncode:
                raise SystemExit(f"{label} collection failed; stderr withheld")
    rows = [
        json.loads(line)
        for line in (destination / f"{sample:03d}-server.jsonl")
        .read_text()
        .splitlines()
    ]
    records = [json.loads(line) for line in result.stdout.splitlines()]
    for record in records:
        dispatches = [
            r
            for r in rows
            if r["event"] == "target dispatched"
            and record["before_us"] - 100000
            <= r["journal_at_us"]
            <= record["received_at_unix_ms"] * 1000 + 10000
        ]
        if not dispatches:
            raise SystemExit("target dispatch missing from collected window")
        record["target_digest"] = dispatches[0 if record["phase"] != "rebind" else -1][
            "target_digest"
        ]
    operations.write_text("".join((json.dumps(r) + "\n" for r in records)))
    print(
        json.dumps(
            {
                "vm": vm,
                "cycle": sample,
                "outcome": "collected",
                "operations": len(records),
            }
        ),
        flush=True,
    )
PY
