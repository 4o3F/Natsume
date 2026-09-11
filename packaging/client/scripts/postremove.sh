#!/bin/sh
set -eu

case "${1:-}" in
remove | purge) ;;
*) exit 0 ;;
esac

# Dependencies may already be absent when postrm runs. Image-owned PAM/GDM,
# templates, accounts, device identity and Home history survive both actions.
if [ "$1" = purge ] && [ -r /usr/share/debconf/confmodule ]; then
  . /usr/share/debconf/confmodule
  # PURGE takes no arguments; do not forward dpkg's action to debconf.
  # shellcheck disable=SC2119
  db_purge
fi

if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi
