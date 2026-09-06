# Helper crash recovery (R05)

Run on a disposable native systemd VM (253 or newer) with the current Client Deb,
a reachable test Server, and a provisioned contestant account and Home template.
Use a snapshot separate from the package lifecycle test. Record the Deb checksum,
OS, kernel, systemd version and journal interval. These tests stop services and can
terminate sessions or reset Home when testing pending operations. Restore the VM
snapshot when finished. A private D-Bus test or unit-file verification alone is
not evidence that the packaged systemd recovery path works.

Define these names in each administrator shell, including after restoring a snapshot:

```bash
helper=natsume-privileged-helper.service
daemon=natsume-device-daemon.service
```

## One unexpected crash

Start with an enrolled, connected Device. Record its current binding, Session/Home
epochs and Actual freshness through the test Server. Keep Caddy running throughout
the crash injection; killing the Daemon instead would test a different path.

```bash
sudo systemctl reset-failed "$helper" "$daemon"
sudo systemctl start "$daemon"
systemctl show "$helper" -p Type -p BusName -p Restart -p RestartUSec \
  -p StartLimitIntervalUSec -p StartLimitBurst -p MainPID -p NRestarts
before_pid=$(systemctl show "$helper" -p MainPID --value)
before_restarts=$(systemctl show "$helper" -p NRestarts --value)
before_daemon=$(systemctl show "$daemon" -p InvocationID --value)
test "$before_pid" -gt 0
sudo busctl --system call org.freedesktop.DBus /org/freedesktop/DBus \
  org.freedesktop.DBus GetNameOwner s org.natsume.Privileged1
sudo systemctl kill --kill-whom=main --signal=SIGKILL "$helper"

for attempt in {1..150}; do
  after_pid=$(systemctl show "$helper" -p MainPID --value)
  if test "$after_pid" -gt 0 && test "$after_pid" != "$before_pid" &&
    systemctl is-active --quiet "$helper"; then
    break
  fi
  sleep 0.2
done
test "$after_pid" -gt 0
test "$after_pid" != "$before_pid"
systemctl is-active --quiet "$helper"
test "$(systemctl show "$helper" -p NRestarts --value)" -gt "$before_restarts"
sudo busctl --system call org.freedesktop.DBus /org/freedesktop/DBus \
  org.freedesktop.DBus GetNameOwner s org.natsume.Privileged1
systemctl show "$daemon" -p ActiveState -p InvocationID
sudo journalctl -u "$helper" -u "$daemon" -u natsume-caddy.service \
  --since '-2 minutes' --no-pager
```

Require the packaged values `Type=dbus`, the exact `org.natsume.Privileged1` bus
name, `Restart=on-failure`, a two-second restart delay, a 60-second interval and
burst 5. The Helper must acquire a different unique D-Bus owner and PID. No manual
start/restart command may be used after SIGKILL to obtain the recovery result.
Record whether the Daemon retained `before_daemon` or systemd restarted it; either
way it must reconnect/report fresh Actual without operator intervention. Do not
count an old cached Server observation as recovery.

Two seconds of downtime may fall entirely between observations. To exercise the
unavailable path deterministically, repeat on a fresh snapshot with only this
temporary runtime drop-in, using a 20-second restart delay:

```bash
dropin=/run/systemd/system/natsume-privileged-helper.service.d/90-r05-delay.conf
sudo test ! -e "$dropin"
sudo install -d -m 0755 /run/systemd/system/natsume-privileged-helper.service.d
printf '[Service]\nRestartSec=20s\n' | sudo tee "$dropin" >/dev/null
sudo systemctl daemon-reload
sudo systemctl kill --kill-whom=main --signal=SIGKILL "$helper"
```

Observe the Server connection and the local gateway during the gap. Once the
Daemon detects an observation failure and leaves its active lease, Caddy must
serve BLOCKED (503) or have no listener before reconnecting. If BLOCKED cannot be
confirmed, Daemon failure must cause the existing Caddy hard-stop/bootstrap path.
Do not dump Caddy's live configuration, which contains credentials. After recovery,
the new lease must produce a fresh snapshot before Server convergence is accepted.
Remove only `90-r05-delay.conf` and run `systemctl daemon-reload` afterwards.

## Pending work survives the new owner

Use separate snapshots for Session termination and Home reset. Through the test
Server, request one new epoch and inject SIGKILL after the pending operation has
been durably recorded but before completion. If the operation finishes too quickly
to hit that boundary, record the attempt as unexercised; a crash after completion
does not test pending recovery.

- Session: capture `session-completion.json` before the crash. If a new graphical
  session appears, the pending `boot_id` and `logind_session_id` must remain those
  captured originally. The old terminate request must not terminate the new
  session. Completion may advance only after the exact pending transition has
  succeeded; the new session's lock level follows the current target separately.
- Home: keep the contestant logged out, record the Helper's progress for the chosen
  epoch, and verify that restart recovers/verifies that same generation. Require
  fresh mount verification before accepting `completed_reset_epoch`. Follow the
  host and contestant visibility checks in [Home acceptance](home-reset.md).
- During absence, pending/failed state must not turn into successful completion.
  After recovery, replaying the same completed target must not repeat its effect.

The automated `helper_owner_replacement_preserves_pending_session_and_home_work`
test exercises these Client rules on an isolated D-Bus with a replacement service
owner. This VM step additionally verifies the real Helper, logind, mounts and
packaged supervision.

## Consecutive startup failures are bounded

On a fresh snapshot, stop the Daemon and Helper. Install one temporary runtime
drop-in that makes Helper startup fail without touching Home or session state:

```bash
sudo systemctl stop "$daemon" "$helper"
dropin=/run/systemd/system/natsume-privileged-helper.service.d/90-r05-failure.conf
sudo test ! -e "$dropin"
sudo install -d -m 0755 /run/systemd/system/natsume-privileged-helper.service.d
printf '[Service]\nExecStart=\nExecStart=/usr/bin/false\n' |
  sudo tee "$dropin" >/dev/null
sudo systemctl daemon-reload
sudo systemctl reset-failed "$helper" "$daemon"
sudo systemctl start "$helper" # Expected to fail; automatic retries follow.
```

Within 30 seconds require `Result=start-limit-hit`, with five failed process
starts in the journal and no D-Bus owner. The limit counts the first start as well
as retries. Record `NRestarts`, wait beyond the 60-second interval, then verify no
new automatic attempt occurred. The expired interval does not itself schedule a
restart. The Daemon must remain stopped and Caddy must remain on its blocked
bootstrap configuration.

Remove only this fault injection, then explicitly restore service:

```bash
sudo rm /run/systemd/system/natsume-privileged-helper.service.d/90-r05-failure.conf
sudo systemctl daemon-reload
sudo systemctl reset-failed "$helper" "$daemon"
sudo systemctl start "$daemon"
systemctl is-active --quiet "$helper" "$daemon"
```

## Maintenance stop remains stopped

With the normal two-second policy restored, record Helper `NRestarts`, then run
`systemctl stop natsume-device-daemon.service natsume-privileged-helper.service`.
After at least five seconds, both units must remain inactive, the Helper must have
no D-Bus owner and `NRestarts` must not increase. Verify that the existing Daemon
stop hook leaves Caddy on blocked bootstrap. Explicitly start the Daemon when
maintenance is finished; its dependencies must bring Helper up before startup
identity collection. A deliberate stop is not an unexpected-crash test.
