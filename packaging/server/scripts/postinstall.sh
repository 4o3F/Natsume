#!/bin/sh
set -eu

# Highlight severities only on a terminal; logs and NO_COLOR stay plain text.
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
  hint_style='\033[1;36m' warn_style='\033[1;33m' fail_style='\033[1;31m' plain_style='\033[0m'
else
  hint_style='' warn_style='' fail_style='' plain_style=''
fi

hint() {
  printf '%b%s%b\n' "$hint_style" "natsume-server: $*" "$plain_style" >&2
}

warn() {
  printf '%b%s%b\n' "$warn_style" "natsume-server: warning: $*" "$plain_style" >&2
}

fail() {
  printf '%b%s%b\n' "$fail_style" "natsume-server: error: $*" "$plain_style" >&2
  exit 1
}

case "${1:-configure}" in
configure) ;;
abort-*) exit 0 ;;
*) exit 0 ;;
esac

systemd-sysusers /usr/lib/sysusers.d/natsume-server.conf >/dev/null ||
  fail 'failed to create required package user'
systemd-tmpfiles --create /usr/lib/tmpfiles.d/natsume-server.conf >/dev/null ||
  fail 'failed to create required state directories'

# Deployment owns the configuration and public trust files.
missing_input=false
for path in /etc/natsume-server/config.toml /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt; do
  if [ ! -e "$path" ]; then
    missing_input=true
    warn "deployment input $path is missing; service startup is deferred"
  elif [ ! -f "$path" ] || [ ! -s "$path" ] || [ ! -r "$path" ]; then
    fail "deployment input $path must be a nonempty readable file"
  fi
done
if [ "$missing_input" = true ]; then
  hint "run 'sudo natsume-server-setup' to perform first installation"
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 ||
    warn 'systemd daemon-reload deferred'
fi
