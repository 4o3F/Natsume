#!/usr/bin/env bash
# Run as root on the isolated Server host; PID changes start a new CPU interval.
set -euo pipefail
interval="$1"
port="$2"
[[ "$port" =~ ^[0-9]+$ ]]
test "$port" -ge 1 && test "$port" -le 65535
ticks=$(getconf CLK_TCK)
page=$(getconf PAGESIZE)
previous_pid=0
previous_time=0
previous_user=0
previous_sys=0
read -r origin _ < /proc/uptime
index=0
printf '%s\n' 'ts_us,monotonic_s,pid,user_ticks,sys_ticks,ticks_per_second,cpu_user_percent,cpu_sys_percent,rss_bytes,cgroup_memory_current_bytes,cgroup_memory_peak_bytes,threads,open_fds,tcp_8443_established'
while true; do
  stamp=$(date +%s%6N)
  read -r now _ < /proc/uptime
  pid=$(systemctl show natsume-bench -p MainPID --value)
  group=$(systemctl show natsume-bench -p ControlGroup --value)
  user=0; system=0; rss=0; threads=0; fds=0; current=0; peak=0; cpu_user=''; cpu_sys=''
  if test "$pid" -gt 0 && test -r "/proc/$pid/stat"; then
    stat=$(cat "/proc/$pid/stat" 2>/dev/null) || continue
    read -ra fields <<< "${stat##*) }"
    user=${fields[11]}; system=${fields[12]}; threads=${fields[17]}; rss=$((fields[21] * page))
    fds=$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l) || fds=0
    current=$(cat "/sys/fs/cgroup$group/memory.current" 2>/dev/null) || current=0
    peak=$(cat "/sys/fs/cgroup$group/memory.peak" 2>/dev/null) || peak=0
    if test "$pid" = "$previous_pid"; then
      read -r cpu_user cpu_sys < <(awk -v t="$now" -v p="$previous_time" -v u="$user" -v pu="$previous_user" -v s="$system" -v ps="$previous_sys" -v hz="$ticks" 'BEGIN { if (t>p) printf "%.6f %.6f\n",(u-pu)/hz/(t-p)*100,(s-ps)/hz/(t-p)*100; else print "0 0" }')
    fi
  fi
  connections=$(ss -Htn state established sport = ":$port" | wc -l)
  printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' "$stamp" "$now" "$pid" "$user" "$system" "$ticks" "$cpu_user" "$cpu_sys" "$rss" "$current" "$peak" "$threads" "$fds" "$connections"
  previous_pid=$pid; previous_time=$now; previous_user=$user; previous_sys=$system
  index=$((index + 1))
  read -r finished _ < /proc/uptime
  pause=$(awk -v origin="$origin" -v sample_index="$index" -v interval="$interval" -v now="$finished" 'BEGIN {delay=origin+sample_index*interval-now; printf "%.3f", (delay>0?delay:0)}')
  sleep "$pause"
done
