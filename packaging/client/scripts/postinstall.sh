#!/bin/sh
set -eu

# Highlight severities only on a terminal; logs and NO_COLOR stay plain text.
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
  warn_style='\033[1;33m' fail_style='\033[1;31m' plain_style='\033[0m'
else
  warn_style='' fail_style='' plain_style=''
fi

warn() {
  printf '%b%s%b\n' "$warn_style" "natsume-client: warning: $*" "$plain_style" >&2
}

fail() {
  printf '%b%s%b\n' "$fail_style" "natsume-client: error: $*" "$plain_style" >&2
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
    warn "deployment input $path is missing; service startup is deferred"
  elif [ ! -f "$path" ] || [ ! -s "$path" ] || [ ! -r "$path" ]; then
    fail "deployment input $path must be a nonempty readable file"
  fi
done

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 ||
    warn 'systemd daemon-reload deferred'
fi
