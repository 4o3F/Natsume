#!/bin/sh
set -eu

case "${1:-configure}" in
configure) ;;
abort-*) exit 0 ;;
*) exit 0 ;;
esac

systemd-sysusers /usr/lib/sysusers.d/natsume-server.conf >/dev/null ||
  {
    printf '%s\n' 'natsume-server: failed to create required package user' >&2
    exit 1
  }
systemd-tmpfiles --create /usr/lib/tmpfiles.d/natsume-server.conf >/dev/null ||
  {
    printf '%s\n' 'natsume-server: failed to create required state directories' >&2
    exit 1
  }

# Deployment owns the configuration and public trust files.
for path in /etc/natsume-server/config.toml /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt; do
  if [ ! -e "$path" ]; then
    printf 'natsume-server: deployment input %s is missing; service startup is deferred\n' "$path" >&2
  elif [ ! -f "$path" ] || [ ! -s "$path" ] || [ ! -r "$path" ]; then
    printf 'natsume-server: deployment input %s must be a nonempty readable file\n' "$path" >&2
    exit 1
  fi
done

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 ||
    printf '%s\n' 'natsume-server: systemd daemon-reload deferred' >&2
fi
