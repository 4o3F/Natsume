#!/bin/sh
# Run as root on the VM host, inside a transient systemd service.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
umask 077

if [ "$#" -ne 7 ]; then
    echo "Usage: $0 ISO WORK_DIR CPU_NODE MEMORY_NODE DISK_GIB OVMF_CODE OVMF_VARS" >&2
    exit 2
fi
iso=$1
work=$2
cpu_node=$3
memory_node=$4
disk_gib=$5
firmware=$6
vars=$7
case "$iso:$work:$firmware:$vars" in
    *[!a-zA-Z0-9_./:-]*) echo 'Paths must use letters, digits, _, ., / or -' >&2; exit 2 ;;
esac
for number in "$cpu_node" "$memory_node" "$disk_gib"; do
    case "$number" in ''|*[!0-9]*) exit 2 ;; esac
done
test "$(id -u)" = 0
test -r /dev/kvm
test -f "$iso"
test "$disk_gib" -ge 32
# Check every ancestor before creating files that may hold deployment secrets.
parent=$work
while :; do
    test -d "$parent"
    test ! -L "$parent"
    test "$(stat -c %u "$parent")" = 0
    mode=$(stat -c %a "$parent")
    test $((0$mode & 0022)) -eq 0
    test "$parent" != / || break
    parent=$(dirname "$parent")
done
test "$(stat -c %a "$work")" = 700
test ! -e "$work/template.qcow2"
test ! -e "$work/install"
test "$(df -Pk "$work" | awk 'NR==2 {print $4}')" -ge $((disk_gib * 1024 * 1024 + 5 * 1024 * 1024))
mkdir "$work/install"
run=$work/install
mkdir "$run/media"
mounted=false
logger_pid=
# shellcheck disable=SC2317 # Called by the EXIT trap.
cleanup() {
    if [ -n "$logger_pid" ]; then
        kill "$logger_pid" 2>/dev/null || true
        wait "$logger_pid" 2>/dev/null || true
    fi
    if [ "$mounted" = true ]; then umount "$run/media"; fi
}
trap 'cleanup' EXIT
trap 'exit 143' TERM INT
mount -o loop,ro,nosuid,nodev,noexec "$iso" "$run/media"
mounted=true
# Read the standard Ubuntu GRUB entry as data; never execute its contents.
entry=$(awk '
    /menuentry "Automated Install"/ {inside=1; next}
    inside && /}/ {exit}
    inside && ($1=="linux" || $1=="initrd") {print}
' "$run/media/boot/grub/grub.cfg")
kernel=$(printf '%s\n' "$entry" | awk '$1=="linux" {print $2}')
initrd=$(printf '%s\n' "$entry" | awk '$1=="initrd" {print $2}')
test "$kernel" = /casper/vmlinuz
test "$initrd" = /casper/initrd
args=$(printf '%s\n' "$entry" | sed -n 's/^[[:space:]]*linux[[:space:]][^[:space:]]*[[:space:]]*//p')
case " $args " in *' autoinstall '*) ;; *) exit 2 ;; esac
case "$args" in *'---'*) ;; *) exit 2 ;; esac
args=$(printf '%s\n' "$args" | sed 's/---/console=tty0 console=ttyS0,115200n8 noprompt ---/')
cp "$run/media$kernel" "$run/vmlinuz"
cp "$run/media$initrd" "$run/initrd"
umount "$run/media"
mounted=false
cp "$vars" "$run/OVMF_VARS.fd"
qemu-img create -f qcow2 "$work/template.qcow2" "${disk_gib}G"
cpus=$(cat "/sys/devices/system/node/node$cpu_node/cpulist")
uuid=$(cat /proc/sys/kernel/random/uuid)
printf '%s\n' "$uuid" > "$run/uuid"
mkfifo "$run/serial.pipe"
# Installer output may echo deployment secrets. Persist only fixed milestones.
awk '
    {lines++}
    /Linux version/ {print "kernel-started"}
    /subiquity/ && !seen {print "installer-console-active"; seen=1}
    /Installation complete|install complete/ {print "installation-complete-message"}
    /reboot: Restarting system/ {print "guest-reboot-requested"}
    /Power down/ {print "guest-poweroff-requested"}
    lines % 100 == 0 {printf "serial-lines=%d\n", lines}
    {fflush()}
' < "$run/serial.pipe" >> "$run/serial.log" &
logger_pid=$!
printf 'starting QEMU: CPU node %s (%s), memory node %s\n' "$cpu_node" "$cpus" "$memory_node"
status=0
numactl --physcpubind="$cpus" --membind="$memory_node" \
    qemu-system-x86_64 -name natsume-template-install \
    -machine q35 -accel kvm -cpu host -smp 2 -m 4096 \
    -drive "if=pflash,format=raw,readonly=on,file=$firmware" \
    -drive "if=pflash,format=raw,file=$run/OVMF_VARS.fd" \
    -drive "if=none,id=os,format=qcow2,file=$work/template.qcow2" \
    -device "virtio-blk-pci,drive=os,serial=template-$uuid" \
    -drive "media=cdrom,format=raw,readonly=on,file=$iso" \
    -smbios "type=1,uuid=$uuid" -smbios "type=2,serial=template-$uuid" \
    -netdev user,id=net0,hostfwd=tcp:127.0.0.1:@BENCH_TEMPLATE_SSH_PORT@-:22 \
    -device virtio-net-pci,netdev=net0 \
    -device virtio-vga -display none -vnc 127.0.0.1:@BENCH_TEMPLATE_VNC_DISPLAY@ \
    -monitor "unix:$run/monitor.sock,server=on,wait=off" \
    -kernel "$run/vmlinuz" -initrd "$run/initrd" -append "$args" \
    -serial stdio -no-reboot < /dev/null > "$run/serial.pipe" || status=$?
wait "$logger_pid"
logger_pid=
printf 'QEMU exited: %s; verify installation before sealing template\n' "$status"
exit "$status"
