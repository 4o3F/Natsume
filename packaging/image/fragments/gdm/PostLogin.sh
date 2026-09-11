#!/bin/sh
set -eu

[ "${LOGNAME:-}" = waiting ] || exit 0
/usr/libexec/gdm-runtime-config set daemon AutomaticLoginEnable false
exec /usr/bin/systemctl kill --kill-whom=main --signal=HUP gdm.service
