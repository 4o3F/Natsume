#!/bin/sh
set -eu
systemd-sysusers /usr/lib/sysusers.d/natsume-server.conf >/dev/null 2>&1 || true
systemd-tmpfiles --create /usr/lib/tmpfiles.d/natsume-server.conf >/dev/null 2>&1 || true
systemctl daemon-reload >/dev/null 2>&1 || true
