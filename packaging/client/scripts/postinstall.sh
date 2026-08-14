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

config=/etc/natsume/config.toml
placeholder='# Natsume endpoint is written by postinstall after debconf validation.'
canonical_ip=
canonical_port=
debconf_loaded=0

load_debconf() {
  if [ "$debconf_loaded" -eq 0 ]; then
    [ -r /usr/share/debconf/confmodule ] ||
      fail 'debconf configuration module is unavailable'
    . /usr/share/debconf/confmodule
    debconf_loaded=1
  fi
}

record_debconf_endpoint() {
  load_debconf
  db_set natsume-client/server-ip "$canonical_ip" ||
    fail 'failed to record canonical Server IP in debconf'
  db_set natsume-client/server-port "$canonical_port" ||
    fail 'failed to record canonical Server port in debconf'
}

canonicalize_endpoint() {
  endpoint=$(/usr/bin/natsume-device-daemon --print-canonical-endpoint "$1" "$2") ||
    return 1
  canonical_ip=${endpoint% *}
  canonical_port=${endpoint##* }
  [ -n "$canonical_ip" ] && [ -n "$canonical_port" ] &&
    [ "$endpoint" = "$canonical_ip $canonical_port" ]
}

read_existing_endpoint() {
  existing_ip=$(sed -n 's/^[[:space:]]*ip[[:space:]]*=[[:space:]]*"\([^"]*\)"[[:space:]]*$/\1/p' "$config")
  existing_port=$(sed -n 's/^[[:space:]]*port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\)[[:space:]]*$/\1/p' "$config")
  [ -n "$existing_ip" ] && [ -n "$existing_port" ] &&
    canonicalize_endpoint "$existing_ip" "$existing_port" &&
    [ "$existing_ip" = "$canonical_ip" ] &&
    [ "$existing_port" = "$canonical_port" ]
}

set_endpoint_metadata() {
  chown root:natsume "$1" ||
    fail 'failed to set endpoint config ownership'
  chmod 0640 "$1" ||
    fail 'failed to set endpoint config mode'
}

write_endpoint() {
  tmp=$(mktemp /etc/natsume/.config.toml.XXXXXX)
  trap 'rm -f "$tmp"' EXIT HUP INT TERM
  {
    printf '%s\n' '# Managed by natsume-client package configuration. Contains no secret.'
    printf '%s\n' '[server]'
    printf 'ip = "%s"\n' "$canonical_ip"
    printf 'port = %s\n' "$canonical_port"
  } >"$tmp"
  set_endpoint_metadata "$tmp"
  mv -f "$tmp" "$config"
  trap - EXIT HUP INT TERM
}

env_ip_set=0
env_port_set=0
[ "${NATSUME_SERVER_IP+x}" = x ] && env_ip_set=1
[ "${NATSUME_SERVER_PORT+x}" = x ] && env_port_set=1
[ "$env_ip_set" -eq "$env_port_set" ] ||
  fail 'NATSUME_SERVER_IP and NATSUME_SERVER_PORT must be supplied together'

existing_state=missing
if [ -e "$config" ]; then
  if grep -Fxq "$placeholder" "$config" &&
    ! grep -Eq '^[[:space:]]*(ip|port)[[:space:]]*=' "$config"; then
    existing_state=missing
  elif read_existing_endpoint; then
    existing_state=valid
    existing_canonical_ip=$canonical_ip
    existing_canonical_port=$canonical_port
  else
    existing_state=invalid
  fi
fi

explicit_reconfigure=${DEBCONF_RECONFIGURE:-${DEBIAN_RECONFIGURE:-0}}
if [ "$env_ip_set" -eq 0 ] && [ "$explicit_reconfigure" != 1 ]; then
  case "$existing_state" in
  valid)
    canonical_ip=$existing_canonical_ip
    canonical_port=$existing_canonical_port
    set_endpoint_metadata "$config"
    record_debconf_endpoint
    if command -v systemctl >/dev/null 2>&1; then
      systemctl daemon-reload >/dev/null 2>&1 ||
        printf '%s\n' 'natsume-client: systemd daemon-reload deferred' >&2
    fi
    exit 0
    ;;
  invalid)
    fail 'existing endpoint config is invalid; run explicit reconfigure or provide a paired environment override'
    ;;
  missing) ;;
  esac
fi

if [ "$env_ip_set" -eq 1 ]; then
  candidate_ip=$NATSUME_SERVER_IP
  candidate_port=$NATSUME_SERVER_PORT
else
  load_debconf
  db_get natsume-client/server-ip ||
    fail 'failed to read natsume-client/server-ip from debconf'
  candidate_ip=${RET:-}
  db_get natsume-client/server-port ||
    fail 'failed to read natsume-client/server-port from debconf'
  candidate_port=${RET:-}
fi

if [ -z "$candidate_ip" ] || [ -z "$candidate_port" ]; then
  fail 'Server IP and port are required'
fi
canonicalize_endpoint "$candidate_ip" "$candidate_port" ||
  fail 'Server endpoint validation failed'
write_endpoint
record_debconf_endpoint

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 ||
    printf '%s\n' 'natsume-client: systemd daemon-reload deferred' >&2
fi
