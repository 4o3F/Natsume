#!/bin/sh
# Create a clone offline on the VM host; never print guest file contents.
set -eu
umask 077
if [ "$#" -lt 4 ] || [ "$#" -gt 5 ]; then
    echo 'Usage: clone-vm.sh WORK NAME INDEX AGENT_PACKAGES [TEMPLATE_WORK]' >&2
    exit 2
fi
work=$1 name=$2 index=$3 agent=$4
template_work=${5:-$work}
case "$work" in /*) ;; *) exit 2 ;; esac
case "$agent" in /*) ;; *) exit 2 ;; esac
case "$template_work" in /*) ;; *) exit 2 ;; esac
case "$work:$agent:$template_work" in *[!a-zA-Z0-9_./:-]*) exit 2 ;; esac
case "$name" in ''|*[!a-zA-Z0-9-]*) exit 2 ;; esac
case "$index" in ''|*[!0-9]*) exit 2 ;; esac
test "$index" -ge 1 && test "$index" -le 99
test "$(id -u)" = 0
for directory in "$work" "$template_work"; do
    parent=$directory
    while :; do
        test -d "$parent" && test ! -L "$parent"
        test "$(stat -c %u "$parent")" = 0
        mode=$(stat -c %a "$parent")
        test $((0$mode & 0022)) -eq 0
        test "$parent" != / || break
        parent=$(dirname "$parent")
    done
done
test "$(stat -c %a "$work")" = 700
test "$(stat -c %a "$template_work/template.qcow2")" = 400
test "$(stat -c %a "$template_work/install/OVMF_VARS.fd")" = 400
test -d "$agent"
run=$work/$name
test ! -e "$run"
exec 9>"$work/clone.lock"
flock -n 9
modprobe nbd max_part=16
test ! -e /sys/block/nbd0/pid
mkdir -m 700 "$run" "$run/root"
connected=false mounted=false
# shellcheck disable=SC2317
cleanup() {
    if [ "$mounted" = true ]; then umount "$run/root"; fi
    if [ "$connected" = true ]; then qemu-nbd --disconnect /dev/nbd0 >/dev/null; fi
}
trap cleanup EXIT
trap 'exit 143' INT TERM
qemu-img create -q -f qcow2 -F qcow2 -b "$template_work/template.qcow2" "$run/root.qcow2"
cp "$template_work/install/OVMF_VARS.fd" "$run/OVMF_VARS.fd"
chmod 600 "$run/OVMF_VARS.fd"
qemu-nbd --connect=/dev/nbd0 "$run/root.qcow2"
connected=true
udevadm settle
mount /dev/nbd0p2 "$run/root"
mounted=true
# Empty machine-id requests systemd's first-boot initialization.
test ! -L "$run/root/etc/machine-id"
: > "$run/root/etc/machine-id"
rm -f "$run/root/var/lib/dbus/machine-id"
dpkg --root="$run/root" -i "$agent"/*.deb > "$run/agent-install.log" 2>&1
systemctl --root="$run/root" enable qemu-guest-agent.service > /dev/null 2>&1
test ! -s "$run/root/etc/machine-id"
test ! -e "$run/root/var/lib/dbus/machine-id"
test "$(find "$run/root/var/lib/natsume/identity" "$run/root/var/lib/natsume/keys" "$run/root/var/lib/natsume/control" -type f | wc -l)" -eq 0
umount "$run/root"
mounted=false
qemu-nbd --disconnect /dev/nbd0 >/dev/null
connected=false
cat /proc/sys/kernel/random/uuid > "$run/uuid"
printf '%s\n' "$index" > "$run/index"
printf '%s prepared: empty machine-id, no enrolled identity, independent overlay and UEFI vars\n' "$name"
