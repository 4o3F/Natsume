#!/usr/bin/env bash
# Only the isolated bench snapshot and data tree are modified.
set -euo pipefail
version="$1"
snapshot="$2"
[[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
[[ "$snapshot" =~ ^[a-zA-Z0-9_.-]+\.tar\.gz$ ]]
base="/srv/natsume-bench/$version"
for parent in / /srv /srv/natsume-bench "$base"; do
  test ! -L "$parent"
  test "$(stat -c %u "$parent")" = 0
  mode=$(stat -c %a "$parent")
  test $((8#$mode & 0022)) -eq 0
done
test "$(systemctl is-enabled natsume-server)" = disabled
test "$(systemctl show natsume-server -p MainPID --value)" = 0
cd "$base"
sha256sum -c "$snapshot.sha256"
# Reject archive paths outside data before extracting into root-only staging.
tar -tzf "$snapshot" | awk '$0 !~ /^data\// || $0 ~ /(^|\/)\.\.(\/|$)/ {bad=1} END {exit bad}'
systemctl stop natsume-bench
stage=$(mktemp -d "$base/.restore.XXXXXXXX")
chmod 0700 "$stage"
tar --xattrs --acls --numeric-owner -xpzf "$snapshot" -C "$stage"
test -d "$stage/data"
test ! -L "$stage/data"
if test -e "$base/data" || test -L "$base/data"; then mv -T "$base/data" "$stage/previous"; fi
mv -T "$stage/data" "$base/data"
# Retain the previous service-owned tree inside the root-only staging directory.
printf 'previous bench state retained in %s\n' "$stage"
systemctl start natsume-bench
test "$(systemctl show natsume-bench -p ActiveState --value)" = active
