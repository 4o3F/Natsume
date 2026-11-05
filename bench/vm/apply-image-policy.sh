#!/bin/sh
# Install a committed image policy on an unused experiment clone through QGA.
set -eu
umask 077
test "$#" -eq 3 || { echo 'Usage: apply-image-policy.sh WORK NAME COMMITTED_MODULE_TAR' >&2; exit 2; }
work=$1 name=$2 archive=$3
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
test "$(id -u)" = 0
case "$work" in /*) ;; *) exit 2 ;; esac
case "$name" in ''|*[!a-zA-Z0-9-]*) exit 2 ;; esac
parent=$work
while :; do
 test -d "$parent" && test ! -L "$parent"
 test "$(stat -c %u "$parent")" = 0
 mode=$(stat -c %a "$parent"); test $((0$mode & 0022)) -eq 0
 test "$parent" != / || break
 parent=$(dirname "$parent")
done
task=$(mktemp "$work/policy-probe.XXXXXXXX")
trap 'rm -f "$task"' EXIT
cat > "$task" <<'GUEST'
set -eu
umask 077
trap 'printf "policy_failed_line=%s\n" "$LINENO"' ERR
python3 - <<'PY'
import os,pathlib,stat,json
paths=[pathlib.Path('/'),*pathlib.Path('/').iterdir()]
for base,dirs,_ in os.walk('/etc',followlinks=False):
 paths.append(pathlib.Path(base))
 dirs[:]=[d for d in dirs if not pathlib.Path(base,d).is_symlink()]
seen=set();count=0
for p in sorted(paths):
 if str(p) in seen:continue
 seen.add(str(p));s=p.lstat()
 if stat.S_ISDIR(s.st_mode) and s.st_mode & 0o020:
  print(json.dumps({'audit_directory':str(p),'mode':oct(stat.S_IMODE(s.st_mode))[2:]}));count+=1
print(json.dumps({'group_writable_directory_count':count}))
for p in sorted(pathlib.Path('/etc/natsume').rglob('*')):
 print(json.dumps({'natsume_entry':str(p.relative_to('/etc/natsume')),'mode':oct(stat.S_IMODE(p.lstat().st_mode))[2:]}))
PY
# The experiment explicitly repairs these three known image-build defects.
for directory in / /etc /etc/natsume; do
 test -d "$directory" && test ! -L "$directory"
 chown root:root "$directory"
 chmod 0755 "$directory"
done
rm -f /etc/natsume/contest-reset-policy
install -d -m 700 /var/backups/natsume-bench-step34 /run/natsume-step34
systemctl stop natsume-device-daemon gdm.service natsume-privileged-helper natsume-caddy
for account in teams waiting; do
 loginctl terminate-user "$account" >/dev/null 2>&1 || true
done
python3 - <<'PY'
import subprocess,time,pathlib
for user in ['teams','waiting']:
 uid=subprocess.check_output(['id','-u',user],text=True).strip()
 for _ in range(100):
  r=subprocess.run(['gdbus','call','--system','--dest','org.freedesktop.login1','--object-path','/org/freedesktop/login1','--method','org.freedesktop.login1.Manager.GetUser',uid],capture_output=True,text=True)
  live=subprocess.run(['pgrep','-u',uid],stdout=subprocess.DEVNULL).returncode==0
  if r.returncode and 'org.freedesktop.login1.NoSuchUser' in r.stderr and not live:break
  time.sleep(.1)
 else:raise SystemExit('managed user has not fully exited')
 assert not pathlib.Path('/var/lib/systemd/linger',user).exists()
 print(user+'_fully_exited=true')
PY
if [ ! -f /var/backups/natsume-bench-step34/before.tar ]; then
 tar --one-file-system -cf /var/backups/natsume-bench-step34/before.tar /etc/natsume /var/lib/natsume /var/lib/natsume-privileged /home/teams /home/waiting > /var/backups/natsume-bench-step34/backup.log 2>&1
fi
systemctl stop colord.service
python3 - <<'PY'
import pathlib,json,sqlite3,re
for path in ['/var/spool/cups','/etc/cups/ppd']:
 p=pathlib.Path(path);count=sum(f.is_file() for f in p.rglob('*')) if p.exists() else 0
 print(json.dumps({'legacy_path':path,'file_count':count}))
 assert count==0,'unused-clone precondition failed'
for p in pathlib.Path('/etc/cups').glob('printers.conf*'):
 assert '<Printer' not in p.read_text(errors='replace'),'existing CUPS queues require review'
p=pathlib.Path('/var/cache/cups/job.cache')
if p.exists():
 lines=[s for s in p.read_text(errors='replace').splitlines() if s.strip() and not s.lstrip().startswith('#') and not re.fullmatch(r'NextJobId\s+[0-9]+',s.strip())]
 print(json.dumps({'cups_job_cache_records':len(lines)}));assert not lines
for p in pathlib.Path('/var/lib/colord').rglob('*'):
 if not p.is_file():continue
 if p.suffix=='.db':
  connection=sqlite3.connect('file:'+str(p)+'?mode=ro',uri=True)
  for (table,) in connection.execute("SELECT name FROM sqlite_master WHERE type='table'"):
   count=connection.execute('SELECT COUNT(*) FROM "'+table.replace('"','""')+'"').fetchone()[0]
   print(json.dumps({'colord_database':p.name,'table':table,'row_count':count}));assert count==0
 else:
  assert p.suffix in ['.db-shm','.db-wal'] or p.stat().st_size==0,'persisted ICC data requires review'
print('network_power_legacy_changes=none_unused_clone_settings_preserved')
PY
tar -xf /run/natsume-step34-module.tar -C /run/natsume-step34
module=/run/natsume-step34/modules/310-natsume
for relative in \
 etc/polkit-1/rules.d/00-natsume-managed.rules \
 etc/systemd/logind.conf.d/70-natsume-contest-reset.conf \
 etc/systemd/journald.conf.d/70-natsume-contest-reset.conf \
 etc/tmpfiles.d/00-natsume-login-records.conf \
 etc/logrotate.d/wtmp \
 etc/systemd/system/systemd-update-utmp.service.d/70-natsume-login-records.conf \
 etc/systemd/system/systemd-update-utmp-runlevel.service.d/70-natsume-login-records.conf \
 usr/local/share/dbus-1/system-services/org.cups.cupsd.service \
 usr/local/share/dbus-1/system-services/org.opensuse.CupsPkHelper.Mechanism.service; do
 install -D -o root -g root -m 0644 "$module/rootfs/$relative" "/$relative"
 cmp -s "$module/rootfs/$relative" "/$relative"
done
bash "$module/reset-helper-permissions.sh" > /run/natsume-step34/helper-permissions.log 2>&1
for unit in cups.service cups.socket cups.path cups-browsed.service cups-browsed.socket cups-browsed.path org.cups.cupsd.service cups-pk-helper.service dbus-org.opensuse.CupsPkHelper.Mechanism.service; do
 systemctl mask "$unit" > /dev/null 2>&1
 if systemctl is-active --quiet "$unit"; then systemctl stop "$unit"; fi
 test "$(systemctl is-enabled "$unit" 2>/dev/null || true)" = masked
done
if pgrep -x cups-pk-helper >/dev/null; then pkill -TERM -x cups-pk-helper; fi
systemctl daemon-reload
systemctl restart systemd-logind.service systemd-journald.service
for uid in "$(id -u teams)" "$(id -u waiting)"; do
 for directory in /run/log/journal /var/log/journal; do
  if [ -e "$directory" ]; then
   test ! -L "$directory"
   find "$directory" -xdev \( -type d -o -type f \) -exec setfacl -x "u:$uid" {} +
   find "$directory" -xdev -type d -exec setfacl -x "d:u:$uid" {} +
  fi
 done
done
for record in /run/utmp /var/log/wtmp /var/log/btmp /var/log/lastlog; do
 test ! -L "$record"
 if [ -e "$record" ]; then test -f "$record"; fi
done
systemd-tmpfiles --create /etc/tmpfiles.d/00-natsume-login-records.conf
if [ -f /run/utmp ]; then chown root:utmp /run/utmp; chmod 0660 /run/utmp; fi
systemctl start colord.service
python3 - <<'PY'
import pathlib,subprocess,json,os
for directory in ['/run/log/journal','/var/log/journal']:
 p=pathlib.Path(directory)
 if not p.exists():continue
 for item in p.rglob('*'):
  assert not item.is_symlink(),'unexpected journal link'
  if not item.is_file():continue
  for user in ['teams','waiting']:
   assert subprocess.run(['runuser','-u',user,'--','test','-r',str(item)]).returncode!=0,'old journal is readable'
for record in ['/run/utmp','/var/log/wtmp','/var/log/btmp','/var/log/lastlog']:
 p=pathlib.Path(record)
 if p.exists():
  assert p.stat().st_mode&0o777==0o660
  for user in ['teams','waiting']:
   assert subprocess.run(['runuser','-u',user,'--','test','-r',record]).returncode!=0
for user in ['teams','waiting']:
 for action in ['org.freedesktop.color-manager.create-device','org.freedesktop.NetworkManager.settings.modify.system','org.freedesktop.UPower.PowerProfiles.switch-profile']:
  result=subprocess.run(['runuser','-u',user,'--','sh','-c','exec pkcheck --action-id "$1" --process $$','probe',action],capture_output=True)
  assert result.returncode==1,'managed policy action was not denied'
for destination,path in [('org.cups.cupsd','/org/cups/cupsd'),('org.opensuse.CupsPkHelper.Mechanism','/')]:
 result=subprocess.run(['gdbus','call','--system','--dest',destination,'--object-path',path,'--method','org.freedesktop.DBus.Peer.Ping'],capture_output=True,text=True,timeout=10)
 assert result.returncode!=0,'CUPS D-Bus activation unexpectedly succeeded'
for user in ['teams','waiting']:
 result=subprocess.run(['runuser','-u',user,'--','busctl','--system','call','org.freedesktop.ColorManager','/org/freedesktop/ColorManager','org.freedesktop.ColorManager','CreateDevice','ssa{ss}','natsume-bench-policy-probe','temp','0'],capture_output=True,text=True,timeout=10)
 assert result.returncode!=0 and any(v in result.stderr.lower() for v in ['auth','access denied','permission']),'colord service did not deny mutation'
 for helper in ['/usr/bin/fusermount3','/usr/bin/fusermount','/usr/lib/x86_64-linux-gnu/utempter/utempter']:
  if pathlib.Path(helper).exists():assert subprocess.run(['runuser','-u',user,'--','test','-x',helper]).returncode!=0
# Root retains the read-only hardware/service inspection path.
assert subprocess.run(['busctl','--system','call','org.freedesktop.ColorManager','/org/freedesktop/ColorManager','org.freedesktop.ColorManager','GetDevices'],capture_output=True).returncode==0
print('journal_acl_login_records_managed_policy=verified')
print('cups_dbus_denied_colord_mutation_denied_admin_read_allowed=true')
PY
# Publish only after static policy and unused-clone legacy checks succeed.
for directory in / /etc /etc/natsume; do
 test ! -L "$directory" && test "$(stat -c %u "$directory")" = 0
 mode=$(stat -c %a "$directory"); test $((0$mode & 0022)) -eq 0
done
policy=$(mktemp /etc/natsume/.contest-reset-policy.XXXXXX)
printf '1\n' > "$policy"
chown root:root "$policy"
chmod 0644 "$policy"
mv -T "$policy" /etc/natsume/contest-reset-policy
printf 'image_policy_published=true\n'
GUEST
"$here/qga.sh" "$work" "$name" "$task" "$archive" /run/natsume-step34-module.tar
