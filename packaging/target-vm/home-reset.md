# Home mount acceptance (R01)

Run only on a disposable native systemd VM (253 or newer, for `OpenFile`) with the
current Client Deb installed, the image-provided `contest` account and `/home/contest`, and the immutable template
at `/usr/lib/natsume/home-templates/current/lower`. This resets the contestant Home.
Use a separate snapshot from the package lifecycle test, which ends by purging the
package. Record the Deb checksum, OS, kernel and systemd versions with the results.

Use an administrator terminal in the host mount namespace for the commands below.
Log out every contestant graphical session before Prepare, Apply or Recover and
prevent new logins until Verify succeeds. Keep the Daemon stopped throughout this
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
```

Require `PrivateNetwork=yes`, `PrivateMounts=no` and only `AF_UNIX` in
`RestrictAddressFamilies`, plus the sole
`OpenFile=/proc/1/ns/mnt:host-mount-namespace:read-only` handoff. This lets the Helper
inspect the host namespace while keeping its restricted capability set. The mount
namespace device/inode pairs must be identical; the network namespace pairs must
differ. Choose an unused `reset_epoch` larger
than the queried epoch (or 1 if no progress exists), within `1..=i64::MAX`. The
following example uses 1; replace it if the snapshot already contains progress.

```bash
reset_epoch=1
sudo -u contest touch /home/contest/.natsume-r01-old
test ! -e /usr/lib/natsume/home-templates/current/lower/.natsume-r01-old
helper_call PrepareHomeReset t "${reset_epoch}"
helper_call ApplyHomeReset t "${reset_epoch}"
helper_call VerifyHomeReset t "${reset_epoch}"
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
sudo systemctl start natsume-privileged-helper.service
helper_call QueryHomeReset
helper_call RecoverHomeReset t "${reset_epoch}"
helper_call VerifyHomeReset t "${reset_epoch}"
findmnt --mountpoint /home/contest -o TARGET,FSTYPE,OPTIONS
test ! -e /home/contest/.natsume-r01-old
test -f /home/contest/.natsume-r01-current
```

The marker alone is not proof of a restored mount. Require Verify to return the
same epoch as `Verified`, then repeat the new contestant login checks above.
Archive command output and the Helper journal for each boot. This procedure
validates Helper mount ownership and recovery; it does not replace end-to-end
Server/Daemon reset convergence acceptance.
