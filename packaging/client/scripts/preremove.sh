#!/bin/sh
set -eu

case "${1:-}" in
remove | deconfigure) ;;
*) exit 0 ;;
esac

fail() {
  printf 'natsume-client: %s\n' "$*" >&2
  exit 1
}

# Serialize with the Helper and fixed login entry until services have stopped.
# Offline image roots must not have the running system's /run mounted inside.
if [ -d /run/systemd/system ]; then
  runtime=/run/natsume-privileged
  if [ ! -d "$runtime" ] || [ -L "$runtime" ] ||
    [ "$(stat -c '%u:%a' "$runtime")" != 0:700 ]; then
    fail 'login admission directory is unsafe or unavailable'
  fi
  lock=$runtime/session-mutation.lock
  if [ -e "$lock" ] || [ -L "$lock" ]; then
    if [ ! -f "$lock" ] || [ -L "$lock" ] ||
      [ "$(stat -c '%u:%a' "$lock")" != 0:600 ]; then
      fail 'session mutation lock is unsafe'
    fi
  fi
  umask 077
  exec 9>>"$lock"
  flock -n 9 || fail 'graphical mutation in progress; retry removal after it completes'
fi

state=/var/lib/natsume-privileged/home-reset
if [ -e "$state/login-window" ] || [ -L "$state/login-window" ]; then
  fail 'unfinished Home window; recover with the installed Helper before removal'
fi

if [ -e "$state/progress" ] || [ -L "$state/progress" ]; then
  if [ ! -f "$state/progress" ] || [ -L "$state/progress" ]; then
    fail 'Home progress is unsafe'
  fi
  # Accept only the complete persisted format, never an unfinished or corrupt epoch.
  {
    IFS= read -r epoch && IFS= read -r phase &&
      ! IFS= read -r extra && [ -z "$extra" ]
  } <"$state/progress" || fail 'Home progress is invalid; recover before removal'
  case "$epoch" in
  '' | *[!0-9]*) fail 'Home epoch is invalid' ;;
  esac
  if ! [ "$epoch" -gt 0 ] || ! [ "$epoch" -le 9223372036854775807 ] ||
    [ "$phase" != verified ]; then
    fail 'Home is not verified; recover before removal'
  fi
fi

if [ -d /run/systemd/system ]; then
  require_inactive() {
    active=$(systemctl show --property=ActiveState --value "$1") ||
      fail "cannot inspect $1"
    [ "$active" = inactive ] ||
      fail "$1 is not inactive; stop GDM and log out waiting/teams in maintenance before removal"
  }
  require_inactive display-manager.service
  for user in waiting teams; do
    if uid=$(id -u "$user" 2>/dev/null); then
      require_inactive "user-$uid.slice"
    fi
  done

  systemctl stop natsume-device-daemon.service \
    natsume-session-prepare@waiting.service natsume-session-prepare@contest.service \
    natsume-privileged-helper.service natsume-caddy.service ||
    fail 'failed to stop Natsume services; package files have been retained'
  /usr/lib/natsume/natsume-privileged-helper close-admission ||
    fail 'failed to close contest admission; package files have been retained'
fi
