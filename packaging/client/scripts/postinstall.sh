#!/bin/sh
set -eu

systemd-sysusers /usr/lib/sysusers.d/natsume.conf >/dev/null 2>&1 || true
systemd-tmpfiles --create /usr/lib/tmpfiles.d/natsume.conf >/dev/null 2>&1 || true

server_ip=${NATSUME_SERVER_IP:-}
server_port=${NATSUME_SERVER_PORT:-}

if [ -r /usr/share/debconf/confmodule ]; then
    . /usr/share/debconf/confmodule
    if [ -z "$server_ip" ]; then
        db_get natsume-client/server-ip || true
        server_ip=${RET:-}
    fi
    if [ -z "$server_port" ]; then
        db_get natsume-client/server-port || true
        server_port=${RET:-}
    fi
fi

if [ -z "$server_ip" ] || [ -z "$server_port" ]; then
    echo "natsume-client: Server IP and port are required; use debconf preseed or NATSUME_SERVER_IP/NATSUME_SERVER_PORT" >&2
    exit 1
fi

/usr/bin/natsume-device-daemon --validate-endpoint "$server_ip" "$server_port"

config=/etc/natsume/config.toml
tmp=$(mktemp /etc/natsume/.config.toml.XXXXXX)
trap 'rm -f "$tmp"' EXIT HUP INT TERM
cat >"$tmp" <<EOF
# Managed by natsume-client package configuration. Contains no secret.
[server]
ip = "$server_ip"
port = $server_port
EOF
chown root:natsume "$tmp"
chmod 0640 "$tmp"
mv -f "$tmp" "$config"
trap - EXIT HUP INT TERM

systemctl daemon-reload >/dev/null 2>&1 || true
