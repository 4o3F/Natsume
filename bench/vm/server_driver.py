"""Server R1/R2/R6 controls; all exported remote records are source filtered."""

import datetime
import json
from pathlib import Path
import shlex
import subprocess
import sys
import time

from bench_env import configured
from fault_driver import SSH, HOST, IDs, root, run, guest, target, collect

CFG = configured()
VMROOT = CFG["VM_ROOT"]
RUNROOT = CFG["RUN_ROOT"]


def put(name, location):
    run(
        "B",
        "sudo -n tee "
        + shlex.quote(location)
        + " >/dev/null <<'PUBLIC_SERVER_SCRIPT'\n"
        + CFG.script(root / "bench/vm" / name)
        + "PUBLIC_SERVER_SCRIPT\nsudo -n chmod 755 "
        + shlex.quote(location),
    )


def api(*args):
    return run(
        "B", shlex.join(["/bin/sh", RUNROOT + "/server-api.sh", *map(str, args)])
    ).stdout


def save(out, name, text):
    (out / name).write_text(text)


def stamp():
    return datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%d %H:%M:%S UTC"
    )


def start(vm, identity=None):
    command = [
        "sudo",
        "-n",
        "systemd-run",
        "--unit=natsume-vm-" + vm,
        "--property=Type=exec",
        VMROOT + "/run-vm.sh",
        VMROOT,
        vm,
    ]
    if identity:
        command.append(identity)
    run("B", shlex.join(command) + " >/dev/null 2>&1")


def qga_ready(vm):
    # guest() suppresses failed startup probes; do not inspect guest config contents.
    deadline = time.monotonic() + 100
    while time.monotonic() < deadline:
        try:
            guest(vm, "true")
            return
        except RuntimeError:
            time.sleep(0.5)
    raise RuntimeError("QGA startup timeout")


def poweroff(vm):
    if (
        run(
            "B", f"systemctl show natsume-vm-{vm} -p MainPID --value", False
        ).stdout.strip()
        == "0"
    ):
        return
    try:
        guest(vm, "systemctl poweroff --no-block")
    except RuntimeError:
        pass
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if (
            run(
                "B", f"systemctl show natsume-vm-{vm} -p MainPID --value", False
            ).stdout.strip()
            == "0"
        ):
            return
        time.sleep(0.2)
    raise RuntimeError("VM poweroff timeout")


def snapshot(out, prefix):
    save(out, prefix + "-api.jsonl", api("devices"))
    for vm in [CFG["VM1"], CFG["VM2"]]:
        if (
            run(
                "B", f"systemctl show natsume-vm-{vm} -p MainPID --value", False
            ).stdout.strip()
            != "0"
        ):
            save(
                out,
                prefix + "-" + vm + "-home.json",
                guest(vm, "/bin/sh /run/natsume-bench-fault-state.sh"),
            )


def events(out, prefix, start_us):
    save(
        out,
        prefix + "-operations.jsonl",
        api("events", start_us // 1000, time.time_ns() // 1000000),
    )


def watch(out, name, seconds):
    f = (out / name).open("w")
    p = subprocess.Popen(
        SSH
        + [
            HOST["B"],
            shlex.join(["/bin/sh", RUNROOT + "/server-api.sh", "watch", str(seconds)]),
        ],
        stdout=f,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    return p, f


def wait_ready(vm, mode=None):
    command = ["/bin/sh", RUNROOT + "/target.sh", IDs[vm], "wait"]
    if mode:
        command.append(mode)
    return run("B", shlex.join(command)).stdout


def prepare(out, clone, template, deb, agents):
    assert not (out / "prepared.json").exists()
    assert all(Path(p).is_absolute() for p in [template, deb, agents])
    assert clone not in IDs and clone.replace("-", "").isalnum()
    run(
        "A",
        'test "$(systemctl show natsume-bench -p MainPID --value)" = 0\ntest "$(systemctl show natsume-server -p MainPID --value)" = 0\ntest "$(systemctl is-enabled natsume-server)" = disabled',
    )
    for name in [
        "run-vm.sh",
        "qga.sh",
        "clone-vm.sh",
        "install-client.sh",
        "fault-state.sh",
        "network-link.sh",
    ]:
        put(name, VMROOT + "/" + name)
    for name in ["target.sh", "fault-api.sh", "server-api.sh", "network-operations.sh"]:
        put(name, RUNROOT + "/" + name)
    run(
        "B",
        shlex.join(
            [
                "sudo",
                "-n",
                VMROOT + "/clone-vm.sh",
                VMROOT,
                clone,
                "3",
                agents,
                template,
            ]
        )
        + " >/dev/null",
    )
    run(
        "B",
        shlex.join(
            [
                "sudo",
                "-n",
                "cp",
                VMROOT + "/" + CFG["VM1"] + "/uuid",
                VMROOT + "/" + clone + "/uuid",
            ]
        ),
    )
    # Hardware identity uses VM1's serial prefix while MAC/ports use clone index 3.
    start(clone, CFG["VM1"])
    qga_ready(clone)
    save(
        out,
        "clone-install.txt",
        run(
            "B",
            shlex.join(
                [
                    "sudo",
                    "-n",
                    VMROOT + "/install-client.sh",
                    VMROOT,
                    clone,
                    deb,
                    VMROOT + "/client-debs/dependencies",
                ]
            ),
        ).stdout,
    )
    poweroff(clone)
    run("A", "sudo -n systemctl start natsume-bench")
    for vm in IDs:
        start(vm)
        qga_ready(vm)
        run(
            "B",
            "sudo -n tee "
            + VMROOT
            + "/server-install.sh >/dev/null <<'PUBLIC_INSTALL'\nchmod 700 /run/natsume-bench-fault-state.sh\nPUBLIC_INSTALL\nsudo -n "
            + VMROOT
            + "/qga.sh "
            + VMROOT
            + " "
            + vm
            + " "
            + VMROOT
            + "/server-install.sh "
            + VMROOT
            + "/fault-state.sh /run/natsume-bench-fault-state.sh",
        )
    save(out, "vm1-contest.jsonl", target(CFG["VM1"], "set_foreground", "contest"))
    save(out, "vm2-waiting.jsonl", target(CFG["VM2"], "set_foreground", "waiting"))
    # Synthetic devices deliberately remain unbound; no roster mutation or seat reuse.
    save(out, "window-open.jsonl", api("window", "open"))
    fleet = RUNROOT + "/server-reliability"
    script = f"""umask 077
mkdir -p {fleet}
python3 - <<'SEATS'
from pathlib import Path
Path('{fleet}/seats.csv').write_text('seat_code\\n'+''.join(f'S{{i:05}}\\n' for i in range(10000,10016)))
SEATS
sudo -n systemd-run --unit=natsume-bench-server-loadgen --property=KillSignal=SIGINT --property=TimeoutStopSec=30 --property=RuntimeMaxSec=1500 --uid=$(id -u) --gid=$(id -g) {CFG["AUTH_DIR"]}/natsume-loadgen run --server {CFG["SERVER_URL"]} --trust-root {CFG["AUTH_DIR"]}/control-ca.crt --state-dir {fleet}/state --seat-plan {fleet}/seats.csv --output-dir {fleet}/events --devices 16 --seed 2026110415 --duration 1400s --ramp 0s --logo-interval 0s >/dev/null 2>&1
"""
    run("B", script)
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        devices = [json.loads(x) for x in api("devices").splitlines()]
        if len(devices) == 18 and all(
            d["connection_state"] == "active" for d in devices
        ):
            break
        time.sleep(1)
    else:
        raise RuntimeError("synthetic fleet did not attach")
    save(out, "window-closed.jsonl", api("window", "closed"))
    save(
        out,
        "prepared.json",
        json.dumps(
            {
                "clone": clone,
                "devices": 18,
                "synthetic_devices": 16,
                "at_us": time.time_ns() // 1000,
            }
        )
        + "\n",
    )
    snapshot(out, "before")


def wait_server_recovery():
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        rows = [json.loads(x) for x in api("devices").splitlines()]
        vms = [d for d in rows if d["device_id"] in IDs.values()]
        if len(vms) == 2 and all(
            d["connection_state"] == "active"
            and all(
                d[k]["status"] == "converged"
                for k in ["binding", "gateway", "session_control", "home"]
            )
            and d["session_control"].get("contest_ready")
            for d in vms
        ):
            return "".join(json.dumps(d) + "\n" for d in vms)
        time.sleep(0.25)
    raise RuntimeError("VM resources did not recover")


def r1(out):
    for trial in [1, 2]:
        prefix = f"{trial:02d}"
        if (out / (prefix + "-operations.jsonl")).exists():
            continue
        assert not (out / (prefix + "-restart.jsonl")).exists()
        snapshot(out, prefix + "-before")
        since = stamp()
        start_us = time.time_ns() // 1000
        p, f = watch(out, prefix + "-watch.jsonl", 65)
        time.sleep(1)
        server_data = shlex.quote(CFG["SERVER_DATA"])
        script = (
            """sudo -n python3 - """
            + server_data
            + """ <<'PUBLIC_RESTART'
import json,pathlib,sqlite3,subprocess,sys,time
start=time.monotonic();at=time.time_ns()//1000
subprocess.run(['systemctl','kill','-s','KILL','natsume-bench'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
subprocess.run(['systemctl','stop','natsume-bench'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
assert subprocess.check_output(['systemctl','show','natsume-bench','-p','MainPID','--value'],text=True).strip()=='0'
print(json.dumps({'kind':'server_kill','at_us':at,'monotonic_us':int(start*1e6)}),flush=True)
for path in sorted(pathlib.Path(sys.argv[1]).rglob('*.db')):
    connection=sqlite3.connect(path.as_uri()+'?mode=ro',uri=True)
    connection.execute('PRAGMA query_only=ON')
    rows=connection.execute('PRAGMA integrity_check').fetchall()
    print(json.dumps({'kind':'integrity_check','result':'ok' if rows==[('ok',)] else 'failed'}),flush=True)
    assert rows==[('ok',)]
    connection.close()
time.sleep(max(0,5-(time.monotonic()-start)))
before=time.time_ns()//1000
subprocess.run(['systemctl','start','natsume-bench'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
print(json.dumps({'kind':'server_start','at_us':before,'completed_at_us':time.time_ns()//1000,'monotonic_us':time.monotonic_ns()//1000}),flush=True)
PUBLIC_RESTART
"""
        )
        save(out, prefix + "-restart.jsonl", run("A", script).stdout)
        save(out, prefix + "-recovered.jsonl", wait_server_recovery())
        snapshot(out, prefix + "-after")
        p.wait(timeout=75)
        f.close()
        events(out, prefix, start_us)
        for vm in IDs:
            collect(out, vm, since, prefix + "-" + vm)
        print(
            json.dumps({"case": "R1", "trial": trial, "result": "collected"}),
            flush=True,
        )


def r2(out):
    vm = CFG["VM2"]
    for seconds in [30, 90]:
        prefix = str(seconds) + "s"
        assert not (out / (prefix + "-link-off.jsonl")).exists()
        save(out, prefix + "-waiting.jsonl", target(vm, "set_foreground", "waiting"))
        snapshot(out, prefix + "-before")
        since = stamp()
        start_us = time.time_ns() // 1000
        p, f = watch(out, prefix + "-watch.jsonl", seconds + 55)
        time.sleep(0.5)
        off = run("B", f"sudo -n /bin/sh {VMROOT}/network-link.sh {vm} off").stdout
        save(out, prefix + "-link-off.jsonl", off)
        deadline = time.monotonic() + seconds
        try:
            time.sleep(5)
            save(
                out, prefix + "-offline-target.jsonl", api("submit", IDs[vm], "contest")
            )
            snapshot(out, prefix + "-offline")
            while time.monotonic() < deadline:
                time.sleep(min(1, deadline - time.monotonic()))
        finally:
            save(
                out,
                prefix + "-link-on.jsonl",
                run("B", f"sudo -n /bin/sh {VMROOT}/network-link.sh {vm} on").stdout,
            )
        save(out, prefix + "-ready.jsonl", wait_ready(vm, "contest"))
        snapshot(out, prefix + "-after")
        p.wait(timeout=seconds + 65)
        f.close()
        events(out, prefix, start_us)
        collect(out, vm, since, prefix)
        print(
            json.dumps({"case": "R2", "seconds": seconds, "result": "collected"}),
            flush=True,
        )


def diagnostic(vm):
    return guest(
        vm,
        """python3 - <<'PUBLIC_DIAG'
import json,pathlib,time
p=pathlib.Path('/run/natsume-diagnostics/latest.json')
d=json.loads(p.read_text());o={'at_us':time.time_ns()//1000,'monotonic_us':time.monotonic_ns()//1000}
for key,allowed in [('stage',{'starting','system_bus','helper','identity_preflight','hardware_identity','local_gateway','control_identity','reconciliation','control_configuration','connecting','handshake','pending_review','authority_installation','active'}),('status',{'starting','retrying','waiting_for_review','denied','active','stopped'})]:
 if d.get(key) in allowed:o[key]=d[key]
f=d.get('failure') or {}
if f.get('code') in {'ENROLLMENT_DENIED','HANDSHAKE_DISCONNECTED','HANDSHAKE_TIMEOUT','ACTIVE_CONNECTION_ENDED'}:o['code']=f['code']
print(json.dumps(o))
PUBLIC_DIAG""",
    )


def r6(out, clone):
    assert clone.replace("-", "").isalnum() and clone not in IDs
    for variant in ["concurrent", "alone"]:
        prefix = variant
        assert not (out / (prefix + "-reviews.jsonl")).exists()
        save(out, prefix + "-closed.jsonl", api("window", "closed"))
        snapshot(out, prefix + "-before")
        start_us = time.time_ns() // 1000
        if variant == "alone":
            poweroff(CFG["VM1"])
        start(clone, CFG["VM1"])
        qga_ready(clone)
        if variant == "concurrent":
            check = """sudo -n python3 - <<'PUBLIC_IDENTITY'
import json,pathlib,subprocess
names=NAMES
commands=[]
for name in names:
 pid=int(subprocess.check_output(['systemctl','show','natsume-vm-'+name,'-p','MainPID','--value'],text=True))
 args=pathlib.Path('/proc',str(pid),'cmdline').read_bytes().decode().split(chr(0))
 values={}
 for key in ['-smbios','-device','-netdev']:
  values[key]=[args[i+1] for i,a in enumerate(args[:-1]) if a==key]
 commands.append(values)
a,b=commands
print(json.dumps({'kind':'hardware_comparison','smbios_equal':a['-smbios']==b['-smbios'],'disk_serial_equal':next(x for x in a['-device'] if x.startswith('virtio-blk-pci'))==next(x for x in b['-device'] if x.startswith('virtio-blk-pci')),'network_distinct':a['-netdev']!=b['-netdev'],'mac_distinct':next(x for x in a['-device'] if x.startswith('virtio-net-pci'))!=next(x for x in b['-device'] if x.startswith('virtio-net-pci'))}))
PUBLIC_IDENTITY
""".replace("NAMES", repr([CFG["VM1"], clone]))
            save(out, "hardware-comparison.json", run("B", check).stdout)
        deadline = time.monotonic() + 75
        while time.monotonic() < deadline:
            reviews = [json.loads(x) for x in api("reviews").splitlines()]
            if reviews:
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("clone did not enter enrollment review")
        save(
            out,
            prefix + "-reviews.jsonl",
            "".join(json.dumps(x) + "\n" for x in reviews),
        )
        save(out, prefix + "-diagnostic.json", diagnostic(clone))
        save(
            out,
            prefix + "-local-gate.json",
            guest(
                clone,
                """python3 - <<'PUBLIC_GATE'
import json,pathlib,subprocess,time,tomllib
with open('/etc/natsume/config.toml','rb') as f:hostname=tomllib.load(f)['site']['gateway_hostname']
r=subprocess.run(['curl','--noproxy','*','--silent','--output','/dev/null','--write-out','%{http_code}','--connect-timeout','1','--max-time','2','--cacert','/etc/natsume/trust/local-origin-ca.crt','--resolve',hostname+':443:127.0.0.1','https://'+hostname+'/'],capture_output=True,text=True)
o={'at_us':time.time_ns()//1000,'http_status':int(r.stdout) if r.stdout.isdigit() else 0,'http_result':{0:'completed',7:'connection_failed',28:'timeout',35:'tls_connect_error',60:'tls_verify_failed'}.get(r.returncode,'other_error'),'permit_present':pathlib.Path('/run/natsume-privileged/contest-permit').exists(),'window_present':pathlib.Path('/var/lib/natsume-privileged/home-reset/login-window').exists()}
p=pathlib.Path('/var/lib/natsume-privileged/home-reset/progress')
if p.exists():
 lines=p.read_text().splitlines()
 if len(lines)==2 and lines[0].isdigit() and lines[1] in {'draining','prepared','applied','verified','recovery_required'}:o.update(reset_epoch=int(lines[0]),phase=lines[1])
print(json.dumps(o))
PUBLIC_GATE""",
            ),
        )

        snapshot(out, prefix + "-pending")
        for review in reviews:
            # Only reject the matching hardware identity, leaving unrelated reviews alone.
            original = next(
                json.loads(x)
                for x in api("devices").splitlines()
                if json.loads(x)["device_id"] == CFG["DEVICE1"]
            )
            assert review["machine_hardware_id"] == original["machine_hardware_id"]
            save(out, prefix + "-deny.jsonl", api("deny", review["review_id"]))
        time.sleep(0.5)
        poweroff(clone)
        save(out, prefix + "-reviews-after.jsonl", api("reviews"))
        events(out, prefix, start_us)
        if variant == "alone":
            start(CFG["VM1"])
            qga_ready(CFG["VM1"])
            run(
                "B",
                f"sudo -n {VMROOT}/qga.sh {VMROOT} {CFG['VM1']} {VMROOT}/server-install.sh {VMROOT}/fault-state.sh /run/natsume-bench-fault-state.sh",
            )
            save(out, prefix + "-original-ready.jsonl", wait_server_recovery())
        snapshot(out, prefix + "-after")
        events(out, prefix + "-including-original-resume", start_us)
        print(
            json.dumps({"case": "R6", "variant": variant, "result": "collected"}),
            flush=True,
        )


def finish(out, clone):
    if run(
        "A", "systemctl show natsume-bench -p MainPID --value", False
    ).stdout.strip() not in ["", "0"]:
        put("server-api.sh", RUNROOT + "/server-api.sh")
        save(out, "final-window.jsonl", api("window", "closed"))
        current = [json.loads(line) for line in api("devices").splitlines()]
        hardware = next(
            d["machine_hardware_id"]
            for d in current
            if d["device_id"] == CFG["DEVICE1"]
        )
        for line in api("reviews").splitlines():
            review = json.loads(line)
            if review["machine_hardware_id"] == hardware:
                api("deny", review["review_id"])
        save(out, "final-reviews.jsonl", api("reviews"))
        for vm in IDs:
            save(
                out,
                "final-" + vm + "-waiting.jsonl",
                target(vm, "set_foreground", "waiting"),
            )
        snapshot(out, "final-ready")
        prepared = out / "before-api.jsonl"
        if prepared.exists():
            disabled = []
            for line in prepared.read_text().splitlines():
                device = json.loads(line)["device_id"]
                if device not in IDs.values():
                    disabled.append(api("disable", device))
            save(out, "synthetic-disabled.jsonl", "".join(disabled))
        save(out, "final-devices.jsonl", api("devices"))
    for vm in [clone, CFG["VM1"], CFG["VM2"]]:
        # Units absent before prepare are harmless.
        state = run(
            "B", f"systemctl show natsume-vm-{vm} -p MainPID --value", False
        ).stdout.strip()
        if state and state != "0":
            if vm in IDs:
                run("B", f"sudo -n /bin/sh {VMROOT}/network-link.sh {vm} on", False)
            poweroff(vm)
    run("B", "sudo -n systemctl stop natsume-bench-server-loadgen.service", False)
    run("A", "sudo -n systemctl stop natsume-bench")
    # Delete only the dedicated, validated root-owned clone after its QEMU exits.
    script = f"""sudo -n python3 - {shlex.quote(VMROOT)} {shlex.quote(clone)} <<'PUBLIC_REMOVE'
import json,pathlib,shutil,subprocess,sys
root=pathlib.Path(sys.argv[1]);name=sys.argv[2];p=root/name
assert name not in {list(IDs)!r} and name.replace('-','').isalnum()
assert subprocess.run(['systemctl','show','natsume-vm-'+name,'-p','MainPID','--value'],capture_output=True,text=True).stdout.strip() in ['', '0']
if p.exists():
 assert p.parent==root and not p.is_symlink() and p.stat().st_uid==0 and p.stat().st_mode&0o022==0
 shutil.rmtree(p)
print(json.dumps({{'kind':'clone_cleanup','directory_absent':not p.exists()}}))
PUBLIC_REMOVE"""
    save(out, "clone-cleanup.json", run("B", script).stdout)
    for host, cmd in [
        (
            "A",
            "systemctl show natsume-bench natsume-server -p Id -p MainPID -p ActiveState; systemctl is-enabled natsume-server || true; sha256sum /etc/systemd/system/natsume-bench.service.d/50-cmp-variant.conf",
        ),
        (
            "B",
            f"systemctl show natsume-vm-{CFG['VM1']} natsume-vm-{CFG['VM2']} natsume-bench-server-loadgen -p Id -p MainPID -p ActiveState; sudo -n du -B1 {VMROOT}/{CFG['VM1']}/root.qcow2 {VMROOT}/{CFG['VM2']}/root.qcow2",
        ),
    ]:
        save(out, "final-" + host + ".txt", run(host, cmd).stdout)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit(
            "Usage: server-run.sh prepare|R1|R2|R6|finish OUTPUT [CLONE TEMPLATE CLIENT_DEB AGENT_DIR]"
        )
    command = sys.argv[1]
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    if command == "prepare":
        prepare(out, *sys.argv[3:])
    elif command == "R1":
        r1(out)
    elif command == "R2":
        r2(out)
    elif command == "R6":
        r6(out, sys.argv[3])
    elif command == "finish":
        finish(out, sys.argv[3])
    else:
        raise SystemExit("invalid case")
