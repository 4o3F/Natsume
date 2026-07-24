#!/bin/sh
set -eu

case "${1:-configure}" in
    configure) ;;
    abort-*) exit 0 ;;
    *) exit 0 ;;
esac

systemd-sysusers /usr/lib/sysusers.d/natsume-server.conf >/dev/null \
    || { printf '%s\n' 'natsume-server: failed to create required package user' >&2; exit 1; }
systemd-tmpfiles --create /usr/lib/tmpfiles.d/natsume-server.conf >/dev/null \
    || { printf '%s\n' 'natsume-server: failed to create required state directories' >&2; exit 1; }

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 \
        || printf '%s\n' 'natsume-server: systemd daemon-reload deferred' >&2
fi
