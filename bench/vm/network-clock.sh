#!/bin/sh
# On B: bound the guest realtime offset without exporting guest state.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
sudo -n python3 - <<'CLOCK'
import socket,time,json
s=socket.socket(socket.AF_UNIX);s.settimeout(2);s.connect('@BENCH_VM_ROOT@/@BENCH_VM2@/guest-agent.sock');f=s.makefile('rwb',buffering=0)
def call(cmd,args):
 ident=time.time_ns();f.write(json.dumps({'execute':cmd,'arguments':args,'id':ident}).encode()+b'\n')
 while True:
  r=json.loads(f.readline().lstrip(b'\xff'))
  if r.get('id')==ident:return r['return']
f.write(b'\xff');call('guest-sync-delimited',{'id':time.time_ns()})
rows=[]
for _ in range(9):
 before=time.time_ns()//1000;g=call('guest-get-time',{})//1000;after=time.time_ns()//1000
 rows.append({'B_before_us':before,'B_after_us':after,'guest_at_us':g,'guest_minus_B_us':g-(before+after)/2,'rtt_us':after-before})
print(json.dumps(min(rows,key=lambda x:x['rtt_us'])))
f.close();s.close()
CLOCK
