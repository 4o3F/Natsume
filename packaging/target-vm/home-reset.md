# Home mount and login maintenance acceptance (R01, C01)

Run only on a disposable native systemd VM (253 or newer, for `OpenFile`) with the
current Client Deb installed, the image-provided `contest` account and `/home/contest`, and the immutable template
at `/usr/lib/natsume/home-templates/current/lower`. This resets the contestant Home.
Use a separate snapshot from the package lifecycle test, which ends by purging the
package. Record the Deb checksum, OS, kernel and systemd versions with the results.

Use an administrator terminal in the host mount namespace for the commands below.
Log out every contestant graphical session before beginning a reset. The packaged
Helper must stop and interlock the display manager until Verify succeeds; do not
substitute a manually blocked login path for this check. Keep the Daemon stopped throughout this
isolated Helper test so it cannot submit a different desired epoch, including after
reboot. Restore the VM snapshot when finished.

## Host and contestant visibility

In the administrator terminal:

```bash
sudo systemctl mask --now natsume-device-daemon.service
sudo systemctl restart natsume-privileged-helper.service
sudo systemctl show natsume-privileged-helper.service \
  -p PrivateNetwork -p PrivateMounts -p OpenFile -p RestrictAddressFamilies -p MainPID
helper_pid=$(systemctl show natsume-privileged-helper.service -p MainPID --value)
sudo stat -Lc '%d:%i %n' /proc/1/ns/mnt "/proc/${helper_pid}/ns/mnt"
sudo stat -Lc '%d:%i %n' /proc/1/ns/net "/proc/${helper_pid}/ns/net"

helper_call() {
  sudo -u natsume busctl --system call org.natsume.Privileged1 \
    /org/natsume/Privileged1 org.natsume.Privileged1 "$@"
}
helper_call QueryHomeReset
sudo systemctl cat display-manager.service
sudo test -f /run/natsume-privileged/home-ready
sudo stat -c '%a %U %G' /run/natsume-privileged
```

Require `PrivateNetwork=yes`, `PrivateMounts=no` and only `AF_UNIX` in
`RestrictAddressFamilies`, plus the sole
`OpenFile=/proc/1/ns/mnt:host-mount-namespace:read-only` handoff. This lets the Helper
inspect the host namespace while keeping its restricted capability set. The mount
namespace device/inode pairs must be identical; the network namespace pairs must
differ. Choose an unused `reset_epoch` larger
than the queried epoch (or 1 if no progress exists), within `1..=i64::MAX`. The
following example uses 1; replace it if the snapshot already contains progress.
Require the display-manager drop-in to contain `Wants` and `After` for the Helper
and the positive `ConditionPathExists=/run/natsume-privileged/home-ready`. Require
the permit directory to be `700 root root`. Begin with the display manager active.

```bash
reset_epoch=1
sudo -u contest touch /home/contest/.natsume-r01-old
test ! -e /usr/lib/natsume/home-templates/current/lower/.natsume-r01-old
helper_call PrepareHomeReset t "${reset_epoch}"
test "$(systemctl show display-manager.service -p ActiveState --value)" = inactive
sudo test ! -e /run/natsume-privileged/home-ready
sudo cat /var/lib/natsume-privileged/home-reset/login-window
# A fresh start must be skipped while the permit is absent.
sudo systemctl start display-manager.service
test "$(systemctl show display-manager.service -p ActiveState --value)" = inactive
helper_call ApplyHomeReset t "${reset_epoch}"
sudo test ! -e /run/natsume-privileged/home-ready
helper_call VerifyHomeReset t "${reset_epoch}"
sudo test -f /run/natsume-privileged/home-ready
sudo test ! -e /var/lib/natsume-privileged/home-reset/login-window
systemctl is-active display-manager.service
findmnt --mountpoint /home/contest -o TARGET,FSTYPE,OPTIONS
test ! -e /home/contest/.natsume-r01-old
sudo -u contest touch /home/contest/.natsume-r01-current
sudo test -f "/var/lib/natsume-privileged/home-reset/generations/${reset_epoch}/upper/.natsume-r01-current"
```

Require Verify to return `tu <reset_epoch> 2` (`Verified`). The host must show
exactly one overlay mount at `/home/contest`, with the fixed template lowerdir
and this epoch's `upper` and `work` directories. Check every command's exit status.

Now log in through the target contestant graphical login. In that **new session's**
terminal, run:

```bash
test ! -e /home/contest/.natsume-r01-old
test -f /home/contest/.natsume-r01-current
findmnt --mountpoint /home/contest -o TARGET,FSTYPE,OPTIONS
```

Require all checks to pass and the same overlay generation to appear. Namespace
IDs alone are insufficient evidence of Home visibility. Log the contestant out
again before the remaining recovery checks.

## Reject an isolated Helper before readiness

In the administrator terminal, install this test-only drop-in:

```bash
sudo install -d -m 0755 /etc/systemd/system/natsume-privileged-helper.service.d
sudo tee /etc/systemd/system/natsume-privileged-helper.service.d/90-r01-test.conf <<'EOF'
[Service]
PrivateMounts=yes
EOF
sudo systemctl daemon-reload
sudo systemctl restart natsume-privileged-helper.service
sudo systemctl show natsume-privileged-helper.service \
  -p ActiveState -p Result -p ExecMainStatus
sudo journalctl -u natsume-privileged-helper.service -n 20 --no-pager
sudo busctl --system call org.freedesktop.DBus /org/freedesktop/DBus \
  org.freedesktop.DBus NameHasOwner s org.natsume.Privileged1
```

The restart must fail, with `ExecMainStatus=1` and the error
`privileged helper must share the host mount namespace` for this invocation.
The Helper now retries failed starts every two seconds, with at most five starts
per 60 seconds. Wait for `Result=start-limit-hit` before recording the final
failed state; every attempt must reject the namespace before acquiring its bus name.
There must be no readiness message from this invocation, and `NameHasOwner` must
return `b false`. Do not count an inspection failure as a successful mismatch test:
it means the VM baseline cannot inspect the namespaces and needs investigation.

Remove only the test drop-in and restart the Helper:

```bash
sudo rm /etc/systemd/system/natsume-privileged-helper.service.d/90-r01-test.conf
sudo systemctl daemon-reload
sudo systemctl reset-failed natsume-privileged-helper.service
sudo systemctl start natsume-privileged-helper.service
helper_call RecoverHomeReset t "${reset_epoch}"
helper_call VerifyHomeReset t "${reset_epoch}"
findmnt --mountpoint /home/contest -o TARGET,FSTYPE,OPTIONS
test ! -e /home/contest/.natsume-r01-old
test -f /home/contest/.natsume-r01-current
```

Require the same epoch to remain `Verified` and the host mount and files to remain
unchanged across the Helper restart. Record the chosen epoch before rebooting.

## Recover after reboot

Reboot the VM. Keep the contestant logged out. In a new administrator terminal,
redefine `helper_call` as above and set `reset_epoch` to the recorded value.

```bash
helper_call QueryHomeReset
helper_call VerifyHomeReset t "${reset_epoch}"
sudo test -f /run/natsume-privileged/home-ready
systemctl is-active display-manager.service
findmnt --mountpoint /home/contest -o TARGET,FSTYPE,OPTIONS
test ! -e /home/contest/.natsume-r01-old
test -f /home/contest/.natsume-r01-current
```

Do not invoke Recover to make this boot check pass: the Helper must restore and
verify the mount automatically before the display manager can start. The marker
alone is not proof of a restored mount. Require Verify to return the
same epoch as `Verified`, then repeat the new contestant login checks above.
Archive command output and the Helper journal for each boot. This procedure
validates Helper mount ownership and recovery; it does not replace end-to-end
Server/Daemon reset convergence acceptance.

## Maintenance-window fault and concurrency checks

Use a fresh snapshot or record a new epoch for each destructive case. Capture
`journalctl -b -o short-monotonic -u natsume-privileged-helper.service -u display-manager.service`,
the `login-window` and `progress` files, and host `findmnt` output. Record the real
display-manager implementation/version as well as Xfce/X11 versions.

1. With an existing contestant graphical session, Prepare must reject without
   stopping the display manager, creating a window or changing Home. Log out and
   repeat Prepare while repeatedly attempting a new graphical login. A login that
   wins before entry must cause rejection; one racing establishment may be
   canceled. Once Prepare succeeds, the service must be inactive, no contestant
   graphical session may remain, and direct starts through both
   `display-manager.service` and its concrete alias must be skipped until Verify.
2. Delay the real display manager's stop in a test-only drop-in (for example,
   `[Service]` with `ExecStopPost=/usr/bin/sleep 10`). Prepare must return a
   transition timeout without creating the new generation or changing progress;
   the window must retain the accepted epoch and the permit must remain absent.
   Once the stop finishes, remove the test drop-in, reload systemd, and retry the
   same epoch. A different epoch must be rejected while the window exists. Also
   test a failed stop job; neither case may switch Home before a confirmed stop.
3. Kill the Helper with SIGKILL after each observable stage: window persistence,
   Prepared, Applied, and Verified before login restoration. For the narrow stages,
   use debugger breakpoints in the matching unstripped binary and record the
   injection point; do not claim coverage from an untimed random kill. After
   systemd restarts the Helper, require the same epoch, a verified host mount,
   and restored login only after verification. Repeat with VM power loss at each
   stage. An incomplete preparation must finish its generation before mounting.
4. After successful Verify, log in and restart the Helper. The exact existing
   graphical session must survive with the same Home generation. Separately,
   begin a reset with the display manager already stopped: Verify must publish
   the permit but must leave the service stopped.
5. Make the template unavailable on a snapshot with an incomplete reset, then
   reboot. Require missing login permit, no graphical login, and no Helper bus
   name indicating successful recovery. Restore the same immutable template,
   clear the Helper start limit, and start the Helper; it must recover the
   recorded epoch before login. A manually written `Verified` marker must not
   make an absent or mismatched mount pass.

These target-VM checks remain required release evidence. Mock D-Bus tests cover
ordering and durable recovery decisions; they do not prove the display manager's
session cleanup, alias drop-in behavior or real overlay visibility.
