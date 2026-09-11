#!/bin/sh
set -eu

fail() {
  printf 'natsume-client: %s\n' "$*" >&2
  exit 1
}

case "${1:-configure}" in
configure) ;;
abort-*) exit 0 ;;
*) exit 0 ;;
esac

systemd-sysusers /usr/lib/sysusers.d/natsume.conf >/dev/null ||
  fail 'failed to create required package users and groups'
systemd-tmpfiles --create /usr/lib/tmpfiles.d/natsume.conf >/dev/null ||
  fail 'failed to create required runtime and state directories'

# Deployment inputs may arrive after a generic package is preinstalled.
# systemd conditions keep the service stopped until all paths exist.
for path in /etc/natsume/config.toml /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt; do
  if [ ! -e "$path" ]; then
    printf 'natsume-client: deployment input %s is missing; service startup is deferred\n' "$path" >&2
  elif [ ! -f "$path" ] || [ ! -s "$path" ] || [ ! -r "$path" ]; then
    fail "deployment input $path must be a nonempty readable file"
  fi
done

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 ||
    printf '%s\n' 'natsume-client: systemd daemon-reload deferred' >&2
fi
