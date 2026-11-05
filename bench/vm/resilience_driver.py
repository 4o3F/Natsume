"""Storage and authority fault probes, using source-filtered observations."""

import datetime
import json
from pathlib import Path
import shlex
import subprocess
import sys
import time
import uuid

from bench_env import configured
from server_driver import (
    run,
    guest,
    CFG,
    IDs,
    SSH,
    HOST,
    root,
    put,
    api,
    save,
    snapshot,
    collect,
    stamp,
    target,
    wait_server_recovery,
    poweroff,
    start,
    qga_ready,
)

CFG = configured()
DATA = CFG["SERVER_DATA"]
VARIANT = str(Path(DATA).parent)
assert Path(DATA).name == "data", (
    "injection requires an explicit isolated data directory"
)


def prepare(out):
    code = """import json,pathlib,shutil,subprocess,sys,tomllib
root=pathlib.Path(sys.argv[1]);data=root/'data';assert root.is_absolute() and root.stat().st_uid==0 and root.stat().st_mode&0o022==0
assert subprocess.check_output(['systemctl','show','natsume-bench','-p','MainPID','--value'],text=True).strip()=='0'
assert subprocess.check_output(['systemctl','show','natsume-server','-p','MainPID','--value'],text=True).strip()=='0'
assert subprocess.run(['systemctl','is-enabled','natsume-server'],capture_output=True,text=True).stdout.strip()=='disabled'
with (root/'config.toml').open('rb') as f:c=tomllib.load(f)
business=pathlib.Path(c['storage']['database']);journal=pathlib.Path(c['storage'].get('journal',business.with_name('natsume-journal.db')))
assert business.is_relative_to(data) and journal.is_relative_to(data)
dest=root/'data-step316-backup';assert not dest.exists()
shutil.copytree(data,dest);dest.chmod(0o700)
s={'business':str(business),'journal':str(journal),'journal_mode':journal.stat().st_mode&0o777,'journal_uid':journal.stat().st_uid,'journal_gid':journal.stat().st_gid}
p=root/'step316-state.json';p.write_text(json.dumps(s));p.chmod(0o600)
print(json.dumps({'event':'backup','files':sum(x.is_file() for x in dest.rglob('*')),'bytes':sum(x.stat().st_size for x in dest.rglob('*') if x.is_file()),'mode':oct(dest.stat().st_mode&0o777)}))
"""
    save(
        out,
        "backup.json",
        run(
            "A",
            shlex.join(["sudo", "-n", "python3", "-", VARIANT])
            + " <<'PUBLIC_PREPARE'\n"
            + code
            + "PUBLIC_PREPARE\n",
        ).stdout,
    )
    run("A", "sudo -n systemctl start natsume-bench")
    for vm in IDs:
        start(vm)
        qga_ready(vm)
    setup(out)


def setup(out):
    put("server-api.sh", CFG["RUN_ROOT"] + "/server-api.sh")
    put("control-probe.sh", CFG["RUN_ROOT"] + "/control-probe.sh")
    put("fault-state.sh", CFG["VM_ROOT"] + "/fault-state.sh")
    for vm in IDs:
        run(
            "B",
            "sudo -n tee "
            + CFG["VM_ROOT"]
            + "/limits-install.sh >/dev/null <<'INSTALL'\nchmod 700 /run/natsume-bench-fault-state.sh\nINSTALL\nsudo -n "
            + CFG["VM_ROOT"]
            + "/qga.sh "
            + CFG["VM_ROOT"]
            + " "
            + vm
            + " "
            + CFG["VM_ROOT"]
            + "/limits-install.sh "
            + CFG["VM_ROOT"]
            + "/fault-state.sh /run/natsume-bench-fault-state.sh",
        )
        save(out, vm + "-waiting.jsonl", target(vm, "set_foreground", "waiting"))
    snapshot(out, "initial")


def pause(vm, action):
    assert action in ["STOP", "CONT"]
    return guest(
        vm,
        "python3 - <<'PUBLIC_SIGNAL'\nimport subprocess,os,signal,json,time\npid=int(subprocess.check_output(['systemctl','show','natsume-device-daemon','-p','MainPID','--value'],text=True));assert pid>1\nos.kill(pid,signal.SIG"
        + action
        + ")\nprint(json.dumps({'event':'daemon_signal','signal':'"
        + action
        + "','pid':pid,'at_us':time.time_ns()//1000}))\nPUBLIC_SIGNAL",
    )


def warnings(out, since, prefix, until=None):
    # Fixed prefixes only: no error strings, SQL, user names or complete journal rows.
    code = r"""import json,re,sys
prefixes=['Device reconciliation will retry','Device transient failure limit reached','journal unavailable; serving in degraded mode','journal batch write failed','journal filesystem budget unavailable']
for line in sys.stdin:
 try:d=json.loads(line)
 except ValueError:continue
 m=d.get('MESSAGE','');p=next((x for x in prefixes if re.match(r'^\s*(INFO|WARN|ERROR)\s+'+re.escape(x)+r'(?:\s|$)',m)),None)
 if not p:continue
 o={'event':p,'journal_at_us':int(d['__REALTIME_TIMESTAMP']),'journal_monotonic_us':int(d['__MONOTONIC_TIMESTAMP'])}
 for k in ['consecutive_failures','attempt']:
  v=re.search(r'\b'+k+r'=(\d+)\b',m)
  if v:o[k]=int(v[1])
 for kind in ['database is locked','database table is locked','attempt to write a readonly database','Read-only file system']:
  if kind in m:o['classification']={'database is locked':'sqlite_busy','database table is locked':'sqlite_locked','attempt to write a readonly database':'sqlite_readonly','Read-only file system':'filesystem_readonly'}[kind];break
 print(json.dumps(o))"""
    args = [
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
    if until:
        args += ["--until", until]
    cmd = shlex.join(args) + " | " + shlex.join(["python3", "-c", code])
    save(out, prefix + "-warnings.jsonl", run("A", cmd).stdout)


def lock(out, seconds):
    path = VARIANT + "/step316-lock.py"
    code = """import json,pathlib,pwd,sqlite3,sys,time,os
root=pathlib.Path(sys.argv[1]);state=json.loads((root/'step316-state.json').read_text());path=state['business'];seconds=int(sys.argv[2]);u=pwd.getpwnam('natsume-server');os.setgroups([]);os.setgid(u.pw_gid);os.setuid(u.pw_uid)
c=sqlite3.connect(path,timeout=5);c.execute('BEGIN IMMEDIATE');print(json.dumps({'event':'lock_acquired','at_us':time.time_ns()//1000,'monotonic_us':time.monotonic_ns()//1000}),flush=True)
time.sleep(seconds);c.rollback();c.close();print(json.dumps({'event':'lock_released','at_us':time.time_ns()//1000,'monotonic_us':time.monotonic_ns()//1000}),flush=True)
"""
    run(
        "A",
        "sudo -n tee "
        + path
        + " >/dev/null <<'PUBLIC_LOCK'\n"
        + code
        + "PUBLIC_LOCK\nsudo -n chmod 700 "
        + path,
    )
    run(
        "A",
        f"sudo -n systemd-run --unit=natsume-bench-sqlite-lock --collect --property=RuntimeMaxSec={seconds + 10} /usr/bin/python3 {path} {VARIANT} {seconds} >/dev/null 2>&1",
    )
    for _ in range(40):
        data = run(
            "A",
            "sudo -n journalctl -u natsume-bench-sqlite-lock -n 8 -o cat --no-pager | python3 -c "
            + shlex.quote(
                "import sys,json\nfor l in sys.stdin:\n try:d=json.loads(l)\n except ValueError:continue\n if d.get('event') in ['lock_acquired','lock_released']:print(json.dumps(d))"
            ),
        ).stdout
        if "lock_acquired" in data:
            return
        time.sleep(0.1)
    raise RuntimeError("write lock was not acquired")


def r3(out):
    setup(out)
    for seconds in [7, 36]:
        prefix = str(seconds) + "s"
        mode = "contest" if seconds == 7 else "waiting"
        snapshot(out, prefix + "-before")
        since = stamp()
        start = time.time_ns() // 1000
        stopped = []
        try:
            for vm in IDs:
                save(out, prefix + "-" + vm + "-stop.json", pause(vm, "STOP"))
                stopped.append(vm)
            for vm in IDs:
                save(
                    out,
                    prefix + "-" + vm + "-submit.jsonl",
                    api("submit", IDs[vm], mode),
                )
            lock(out, seconds)
        finally:
            for vm in stopped:
                save(out, prefix + "-" + vm + "-continue.json", pause(vm, "CONT"))
        time.sleep(seconds + 1)
        save(out, prefix + "-recovered.jsonl", wait_server_recovery())
        snapshot(out, prefix + "-after")
        save(
            out,
            prefix + "-operations.jsonl",
            api("events", start // 1000, time.time_ns() // 1000000),
        )
        cmd = (
            "sudo -n journalctl -u natsume-bench-sqlite-lock --since "
            + shlex.quote(since)
            + " -o cat --no-pager | python3 -c "
            + shlex.quote(
                "import sys,json\nfor l in sys.stdin:\n try:d=json.loads(l)\n except ValueError:continue\n if d.get('event') in ['lock_acquired','lock_released']:print(json.dumps(d))"
            )
        )
        save(out, prefix + "-lock.jsonl", run("A", cmd).stdout)
        warnings(out, since, prefix)
        for vm in IDs:
            collect(out, vm, since, prefix + "-" + vm)
        print(
            json.dumps({"case": "R3", "seconds": seconds, "result": "collected"}),
            flush=True,
        )


def fleet(out):
    folder = CFG["RUN_ROOT"] + "/server-limits-316"
    api("window", "open")
    run(
        "B",
        f"""umask 077
mkdir -p {folder}
python3 - <<'PUBLIC_SEATS'
from pathlib import Path
Path('{folder}/seats.csv').write_text('seat_code\\n'+''.join(f'S{{i:05}}\\n' for i in range(11000,11004)))
PUBLIC_SEATS
sudo -n systemd-run --unit=natsume-bench-limits-loadgen --uid=$(id -u) --gid=$(id -g) --property=KillSignal=SIGINT --property=TimeoutStopSec=20 --property=RuntimeMaxSec=900 {CFG["AUTH_DIR"]}/natsume-loadgen run --server {CFG["SERVER_URL"]} --trust-root {CFG["AUTH_DIR"]}/control-ca.crt --state-dir {folder}/state --seat-plan {folder}/seats.csv --output-dir {folder}/events --devices 4 --seed 2026110417 --duration 850s --ramp 0s --logo-interval 0s >/dev/null 2>&1""",
    )
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        rows = [json.loads(l) for l in api("devices").splitlines()]
        online = [
            d
            for d in rows
            if d["device_id"] not in IDs.values() and d["connection_state"] == "active"
        ]
        if len(online) == 4:
            break
        time.sleep(0.5)
    else:
        raise RuntimeError("fleet enrollment timeout")
    save(out, "fleet-online.jsonl", "".join(json.dumps(x) + "\n" for x in online))
    api("window", "closed")
    run("B", "sudo -n systemctl stop natsume-bench-limits-loadgen")
    metadata = run(
        "B",
        f"""python3 - <<'PUBLIC_IDS'
import json,pathlib
for i in range(4):
 d=json.loads(pathlib.Path('{folder}/state',f'device-{{i:05}}.json').read_text());print(json.dumps({{'index':i,'device_id':d['device_id']}}))
PUBLIC_IDS""",
    ).stdout
    save(out, "fleet-identifiers.jsonl", metadata)


def probe(index, mode, seconds, interval):
    state = CFG["RUN_ROOT"] + f"/server-limits-316/state/device-{index:05}.json"
    cmd = shlex.join(
        [
            "/bin/sh",
            CFG["RUN_ROOT"] + "/control-probe.sh",
            state,
            mode,
            str(seconds),
            str(interval),
        ]
    )
    return subprocess.Popen(
        SSH + [HOST["B"], cmd],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )


def r7(out):
    put("control-probe.sh", CFG["RUN_ROOT"] + "/control-probe.sh")
    if not (out / "fleet-identifiers.jsonl").exists():
        fleet(out)
    since = stamp()
    start = time.time_ns() // 1000
    devices = {
        d["index"]: d["device_id"]
        for d in map(
            json.loads, (out / "fleet-identifiers.jsonl").read_text().splitlines()
        )
    }
    for i, state in [(0, "disabled"), (1, "revoked")]:
        p = probe(i, "flood", 18, 0.05)
        first = p.stdout.readline()
        if not first or json.loads(first).get("event") != "probe_active":
            stdout, _ = p.communicate(timeout=10)
            save(out, f"{i}-probe.jsonl", first + stdout)
            raise RuntimeError("probe did not establish a valid active lease")
        time.sleep(5)
        save(out, f"{i}-lifecycle.jsonl", api("lifecycle", devices[i], state))
        stdout, _ = p.communicate(timeout=25)
        save(out, f"{i}-probe.jsonl", first + stdout)
    snapshot(out, "after")
    save(
        out, "operations.jsonl", api("events", start // 1000, time.time_ns() // 1000000)
    )
    for i in [0, 1]:
        r = subprocess.run(
            [
                "sh",
                str(root / "bench/vm/collect-events.sh"),
                "server",
                HOST["A"],
                since,
                devices[i],
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        save(out, f"{i}-server.jsonl", r.stdout)
    print(json.dumps({"case": "R7", "result": "collected"}), flush=True)


def journal_mount(restore=False):
    action = "restore" if restore else "readonly"
    return run(
        "A",
        f"sudo -n python3 - {VARIANT} {action} <<'PUBLIC_JOURNAL'\n"
        + """import json,pathlib,subprocess,sys
root=pathlib.Path(sys.argv[1]);p=pathlib.Path(json.loads((root/'step316-state.json').read_text())['journal']);assert p.is_relative_to(root)
paths=[pathlib.Path(str(p)+s) for s in ['', '-wal','-shm']]
for q in paths:
 mounted=subprocess.run(['findmnt','-n','--mountpoint',str(q)],capture_output=True).returncode==0
 if sys.argv[2]=='readonly' and q.exists() and not mounted:
  subprocess.run(['mount','--bind',str(q),str(q)],check=True,capture_output=True)
  subprocess.run(['mount','-o','remount,bind,ro',str(q)],check=True,capture_output=True)
 elif sys.argv[2]=='restore' and mounted:subprocess.run(['umount',str(q)],check=True,capture_output=True)
print(json.dumps({'event':'journal_storage','mode':sys.argv[2]}))
PUBLIC_JOURNAL""",
    ).stdout


def r4(out):
    since = stamp()
    run("A", "sudo -n systemctl stop natsume-bench")
    try:
        save(out, "readonly.json", journal_mount())
        run("A", "sudo -n systemctl start natsume-bench")
        time.sleep(3)
        save(out, "degraded.jsonl", api("journal"))
        for vm in IDs:
            save(out, vm + "-contest.jsonl", target(vm, "set_foreground", "contest"))
        save(out, "during.jsonl", api("journal"))
        snapshot(out, "during")
    finally:
        save(out, "restored.json", journal_mount(True))
    deadline = time.monotonic() + 85
    while time.monotonic() < deadline:
        s = api("journal")
        save(out, "recovered.jsonl", s)
        if json.loads(s)["journal"]["status"] == "ready":
            break
        time.sleep(2)
    else:
        raise RuntimeError("journal did not recover")
    put("fault-state.sh", CFG["VM_ROOT"] + "/fault-state.sh")
    for vm in IDs:
        run(
            "B",
            "sudo -n tee "
            + CFG["VM_ROOT"]
            + "/limits-install.sh >/dev/null <<'INSTALL'\nchmod 700 /run/natsume-bench-fault-state.sh\nINSTALL\nsudo -n "
            + CFG["VM_ROOT"]
            + "/qga.sh "
            + CFG["VM_ROOT"]
            + " "
            + vm
            + " "
            + CFG["VM_ROOT"]
            + "/limits-install.sh "
            + CFG["VM_ROOT"]
            + "/fault-state.sh /run/natsume-bench-fault-state.sh",
        )
        save(out, vm + "-waiting.jsonl", target(vm, "set_foreground", "waiting"))
    save(out, "after-new-events.jsonl", api("journal"))
    warnings(out, since, "journal")
    for vm in IDs:
        collect(out, vm, since, "journal-" + vm)
    print(json.dumps({"case": "R4", "result": "collected"}), flush=True)


def readers(out):
    folder = CFG["RUN_ROOT"] + "/server-readers-316"
    active = run(
        "B", "systemctl show natsume-bench-reader-loadgen -p MainPID --value", False
    ).stdout.strip()
    if active not in ["", "0"]:
        return
    api("window", "open")
    try:
        run(
            "B",
            f"""umask 077
mkdir -p {folder}
printf 'seat_code\\nS12000\\nS12001\\n' > {folder}/seats.csv
sudo -n systemd-run --unit=natsume-bench-reader-loadgen --uid=$(id -u) --gid=$(id -g) --property=KillSignal=SIGINT --property=TimeoutStopSec=20 --property=RuntimeMaxSec=900 {CFG["AUTH_DIR"]}/natsume-loadgen run --server {CFG["SERVER_URL"]} --trust-root {CFG["AUTH_DIR"]}/control-ca.crt --state-dir {folder}/state --seat-plan {folder}/seats.csv --output-dir {folder}/events --devices 2 --seed 2026110418 --duration 850s --ramp 0s --logo-interval 0s >/dev/null 2>&1
""",
        )
        wanted = {
            str(uuid.uuid5(uuid.NAMESPACE_OID, "natsume-loadgen:2026110418:" + str(i)))
            for i in range(2)
        }
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            rows = [json.loads(x) for x in api("devices").splitlines()]
            if (
                sum(
                    d["machine_hardware_id"] in wanted
                    and d["connection_state"] == "active"
                    for d in rows
                )
                == 2
            ):
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("normal reader fleet did not attach")
        save(
            out,
            "ordinary-readers.jsonl",
            "".join(
                json.dumps(d) + "\n" for d in rows if d["machine_hardware_id"] in wanted
            ),
        )
    finally:
        api("window", "closed")


def r5(out):
    readers(out)
    put("control-probe.sh", CFG["RUN_ROOT"] + "/control-probe.sh")
    since = stamp()
    start = time.time_ns() // 1000
    for vm in IDs:
        save(out, "baseline-" + vm + ".jsonl", target(vm, "set_foreground", "contest"))
    p = probe(2, "slow", 45, 1)
    # One normal synthetic reader remains online throughout the slow-reader probe.
    normal = probe(3, "flood", 55, 0.5)
    time.sleep(1)
    hardware = str(uuid.uuid5(uuid.NAMESPACE_OID, "natsume-loadgen:2026110417:2"))
    slow_id = next(
        json.loads(line)["device_id"]
        for line in api("devices").splitlines()
        if json.loads(line)["machine_hardware_id"] == hardware
    )
    bf = (out / "burst.jsonl").open("w")
    burst = subprocess.Popen(
        SSH
        + [
            HOST["B"],
            shlex.join(
                [
                    "/bin/sh",
                    CFG["RUN_ROOT"] + "/server-api.sh",
                    "burst",
                    slow_id,
                    "1000",
                ]
            ),
        ],
        stdout=bf,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    time.sleep(1)
    for vm in IDs:
        save(out, "during-" + vm + ".jsonl", target(vm, "set_foreground", "waiting"))
    stdout, _ = p.communicate(timeout=65)
    save(out, "slow.jsonl", stdout)
    stdout, _ = normal.communicate(timeout=65)
    save(out, "normal.jsonl", stdout)
    for vm in IDs:
        save(
            out,
            "matched-post-" + vm + ".jsonl",
            target(vm, "set_foreground", "contest"),
        )
        save(
            out,
            "final-waiting-" + vm + ".jsonl",
            target(vm, "set_foreground", "waiting"),
        )
    assert burst.wait(timeout=30) == 0
    bf.close()
    save(
        out, "operations.jsonl", api("events", start // 1000, time.time_ns() // 1000000)
    )
    for vm in IDs:
        collect(out, vm, since, "latency-" + vm)
    print(json.dumps({"case": "R5", "result": "collected"}), flush=True)


def finish(out):
    run("A", "sudo -n systemctl stop natsume-bench-sqlite-lock.service", False)
    save(out, "journal-unmounted.json", journal_mount(True))
    for unit in [
        "natsume-bench-limits-loadgen",
        "natsume-bench-reader-loadgen",
        "natsume-bench-server-loadgen",
    ]:
        run("B", "sudo -n systemctl stop " + unit, False)
    save(out, "window-closed.jsonl", api("window", "closed"))
    for vm in IDs:
        save(out, vm + "-waiting.jsonl", target(vm, "set_foreground", "waiting"))
        save(out, vm + "-continued.json", pause(vm, "CONT"))
    snapshot(out, "before-shutdown")
    wanted = {
        str(uuid.uuid5(uuid.NAMESPACE_OID, f"natsume-loadgen:{seed}:{i}"))
        for seed, count in [(2026110416, 4), (2026110417, 4), (2026110418, 2)]
        for i in range(count)
    }
    for line in api("devices").splitlines():
        d = json.loads(line)
        if d["machine_hardware_id"] in wanted and d["state"] == "enabled":
            api("lifecycle", d["device_id"], "disabled")
    save(out, "final-devices.jsonl", api("devices"))
    save(out, "final-journal.jsonl", api("journal"))
    for vm in IDs:
        poweroff(vm)
    run("A", "sudo -n systemctl stop natsume-bench")
    code = """import json,pathlib,sqlite3,sys,subprocess
root=pathlib.Path(sys.argv[1]);state=json.loads((root/'step316-state.json').read_text())
assert subprocess.check_output(['systemctl','show','natsume-bench','-p','MainPID','--value'],text=True).strip()=='0'
for key in ['business','journal']:
 p=pathlib.Path(state[key]);assert p.is_relative_to(root)
 assert subprocess.run(['findmnt','-n','--mountpoint',str(p)],capture_output=True).returncode!=0
 c=sqlite3.connect(p.as_uri()+'?mode=ro',uri=True);c.execute('PRAGMA query_only=ON');rows=c.execute('PRAGMA integrity_check').fetchall();assert rows==[('ok',)]
 print(json.dumps({'database':key,'integrity_check':'ok'}));c.close()
p=pathlib.Path(state['journal']);st=p.stat();print(json.dumps({'journal_mode_restored':st.st_mode&0o777==state['journal_mode'],'journal_owner_restored':st.st_uid==state['journal_uid'] and st.st_gid==state['journal_gid']}))
"""
    cmd = (
        shlex.join(["sudo", "-n", "python3", "-", VARIANT])
        + " <<'PUBLIC_INTEGRITY'\n"
        + code
        + "PUBLIC_INTEGRITY\n"
    )
    save(out, "integrity.jsonl", run("A", cmd).stdout)
    save(
        out,
        "final-A.txt",
        run(
            "A",
            "systemctl show natsume-bench natsume-server -p Id -p MainPID -p ActiveState; systemctl is-enabled natsume-server || true; sha256sum /etc/systemd/system/natsume-bench.service.d/50-cmp-variant.conf",
        ).stdout,
    )
    save(
        out,
        "final-B.txt",
        run(
            "B",
            "systemctl show "
            + " ".join("natsume-vm-" + v for v in IDs)
            + " natsume-bench-limits-loadgen natsume-bench-reader-loadgen -p Id -p MainPID -p ActiveState",
        ).stdout,
    )
    print(json.dumps({"stage": "safe_shutdown", "result": "completed"}), flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in [
        "prepare",
        "R3",
        "R7",
        "R4",
        "R5",
        "finish",
    ]:
        raise SystemExit("Usage: resilience-run.sh prepare|R3|R7|R4|R5|finish OUTPUT")
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    globals()[sys.argv[1].lower()](out)
