#!/bin/sh
# Launch from a transient systemd unit on the VM host.
set -eu
if [ "${NATSUME_BENCH_RENDERED:-}" != 1 ]; then
    bench_script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    exec python3 "$bench_script_dir/bench_env.py" exec "$0" "$@"
fi
umask 077
if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    echo 'Usage: run-vm.sh WORK NAME [IDENTITY_NAME]' >&2
    exit 2
fi
work=$1 name=$2
identity_name=${3:-$name}
case "$identity_name" in ''|*[!a-zA-Z0-9-]*) exit 2 ;; esac
case "$work" in /*) ;; *) exit 2 ;; esac
case "$work" in *[!a-zA-Z0-9_./-]*) exit 2 ;; esac
case "$name" in ''|*[!a-zA-Z0-9-]*) exit 2 ;; esac
run=$work/$name
test "$(id -u)" = 0
parent=$run
while :; do
    test -d "$parent" && test ! -L "$parent"
    test "$(stat -c %u "$parent")" = 0
    mode=$(stat -c %a "$parent")
    test $((0$mode & 0022)) -eq 0
    test "$parent" != / || break
    parent=$(dirname "$parent")
done
test "$(stat -c %u:%a "$run")" = 0:700
uuid=$(cat "$run/uuid")
index=$(cat "$run/index")
case "$uuid" in *[!a-f0-9-]*|'') exit 2 ;; esac
case "$index" in *[!0-9]*|'') exit 2 ;; esac
test "$index" -ge 1 && test "$index" -le 99
ssh_port_base=@BENCH_SSH_PORT_BASE@
vnc_display_base=@BENCH_VNC_DISPLAY_BASE@
mac=$(printf '@BENCH_MAC_PREFIX@:%02x' "$index")
exec numactl --physcpubind=@BENCH_CPUSET@ --membind=@BENCH_NUMA_NODE@ \
    qemu-system-x86_64 -name "natsume-$name" \
    -machine q35 -accel kvm -cpu host -smp 2 -m 4096 \
    -drive if=pflash,format=raw,readonly=on,file=@BENCH_OVMF_CODE@ \
    -drive "if=pflash,format=raw,file=$run/OVMF_VARS.fd" \
    -drive "if=none,id=os,format=qcow2,file=$run/root.qcow2" \
    -device "virtio-blk-pci,drive=os,serial=$identity_name-$uuid" \
    -smbios "type=1,uuid=$uuid" -smbios "type=2,serial=$identity_name-$uuid" \
    -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$((ssh_port_base + index))-:22" \
    -device "virtio-net-pci,netdev=net0,mac=$mac" \
    -device virtio-vga -display none -vnc "127.0.0.1:$((vnc_display_base + index))" \
    -monitor "unix:$run/monitor.sock,server=on,wait=off" \
    -chardev "socket,path=$run/guest-agent.sock,server=on,wait=off,id=ga" \
    -device virtio-serial -device virtserialport,chardev=ga,name=org.qemu.guest_agent.0 \
    -serial null
