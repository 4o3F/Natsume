#!/bin/sh
# Manage only the generated compiler workload inside the guest.
set -eu
case "${1:-}" in
 start)
uid=$(id -u teams)
runuser -u teams -- python3 - <<'LOAD_PY'
import pathlib,pwd
p=pathlib.Path(pwd.getpwnam('teams').pw_dir)/'.natsume-bench-c5'
p.mkdir(mode=0o700,exist_ok=False)
source='\n'.join('unsigned f%d(unsigned x) { for (unsigned j=0;j<32;j++) x=(x*1664525u+1013904223u)^(x>>7); return x; }'%i for i in range(1000))
(p/'load.cpp').write_text(source+'\n')
LOAD_PY
runuser -u teams -- env "XDG_RUNTIME_DIR=/run/user/$uid" systemd-run --user --quiet --unit=natsume-bench-c5-load --property=CPUAccounting=yes --property=MemoryAccounting=yes --property=RuntimeMaxSec=330 --property=KillMode=control-group /bin/sh -c 'cd /home/teams/.natsume-bench-c5; while :; do g++ -O2 -c load.cpp -o load.o >/dev/null 2>&1 || exit 1; done'
printf 'compiler_load_started=true\n'
;;
 stop)
uid=$(id -u teams)
runuser -u teams -- env "XDG_RUNTIME_DIR=/run/user/$uid" systemctl --user stop natsume-bench-c5-load
runuser -u teams -- python3 - <<'LOAD_PY'
import pathlib,pwd,shutil
p=pathlib.Path(pwd.getpwnam('teams').pw_dir)/'.natsume-bench-c5'
if p.exists():shutil.rmtree(p)
LOAD_PY
printf 'compiler_load_stopped=true\n'
;;
 *) echo 'Usage: c5-load.sh start|stop' >&2; exit 2 ;;
esac
