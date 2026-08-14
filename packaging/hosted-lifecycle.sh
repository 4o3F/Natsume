#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'hosted-lifecycle: %s\n' "$*" >&2
  exit 1
}

[[ ${EUID} -eq 0 ]] || fail 'run as root on a disposable hosted runner'
[[ ${NATSUME_HOSTED_LIFECYCLE_ACK:-} == hosted-destructive-package-lifecycle ]] ||
  fail 'set NATSUME_HOSTED_LIFECYCLE_ACK=hosted-destructive-package-lifecycle'
[[ -d /run/systemd/system ]] || fail 'hosted runner must be booted with systemd'
if dpkg -s natsume-server >/dev/null 2>&1; then
  fail 'natsume-server is already installed; use a disposable host'
fi
if dpkg -s natsume-client >/dev/null 2>&1; then
  fail 'natsume-client is already installed; use a disposable host'
fi

[[ $# -eq 2 ]] || fail 'usage: hosted-lifecycle.sh <server.deb> <client.deb>'
server_deb=$1
client_deb=$2
[[ -f ${server_deb} ]] || fail "server package is missing: ${server_deb}"
[[ -f ${client_deb} ]] || fail "client package is missing: ${client_deb}"
# apt-get treats arguments without a leading / or ./ as package names, not files.
server_deb=$(realpath -- "${server_deb}")
client_deb=$(realpath -- "${client_deb}")

config=/etc/natsume/config.toml

canonical_endpoint() {
  /usr/bin/natsume-device-daemon --print-canonical-endpoint "$1" "$2"
}

assert_endpoint() {
  local expected ip port
  expected=$(canonical_endpoint "$1" "$2")
  ip=${expected% *}
  port=${expected##* }
  grep -Fxq "ip = \"${ip}\"" "${config}" ||
    fail "config does not contain canonical IP ${ip}"
  grep -Fxq "port = ${port}" "${config}" ||
    fail "config does not contain canonical port ${port}"
}

assert_config_metadata() {
  local actual
  actual=$(stat --format='%U:%G %a' "${config}")
  [[ ${actual} == 'root:natsume 640' ]] ||
    fail "endpoint config metadata is ${actual}, expected root:natsume 640"
}

assert_user() {
  getent passwd "$1" >/dev/null || fail "required user does not exist: $1"
}

assert_group() {
  getent group "$1" >/dev/null || fail "required group does not exist: $1"
}

assert_tmpfiles_path() {
  local path=$1 expected=$2 actual
  [[ -d ${path} ]] || fail "tmpfiles path is missing: ${path}"
  actual=$(stat --format='%U:%G %a' "${path}")
  [[ ${actual} == "${expected}" ]] ||
    fail "tmpfiles path ${path} metadata is ${actual}, expected ${expected}"
}

# Both packages register /etc/natsume/site.toml and the two trust roots as
# conffiles, so the server cycle must fully purge before the client installs.
DEBIAN_FRONTEND=noninteractive apt-get install --yes "${server_deb}"

assert_user natsume-server
assert_group natsume-server

assert_tmpfiles_path /var/lib/natsume-server 'natsume-server:natsume-server 750'
assert_tmpfiles_path /var/lib/natsume-server/keys 'natsume-server:natsume-server 700'
assert_tmpfiles_path /var/lib/natsume-server/backups 'natsume-server:natsume-server 750'
assert_tmpfiles_path /var/log/natsume-server 'natsume-server:natsume-server 750'

systemd-analyze --recursive-errors=no verify \
  /usr/lib/systemd/system/natsume-server.service

DEBIAN_FRONTEND=noninteractive apt-get install --reinstall --yes "${server_deb}"

dpkg --remove natsume-server
[[ -e /etc/natsume/site.toml ]] ||
  fail 'remove deleted server conffile /etc/natsume/site.toml'
[[ -e /etc/natsume/trust/control-ca.crt ]] ||
  fail 'remove deleted server conffile /etc/natsume/trust/control-ca.crt'
[[ -e /etc/natsume/trust/local-origin-ca.crt ]] ||
  fail 'remove deleted server conffile /etc/natsume/trust/local-origin-ca.crt'

dpkg --purge natsume-server
[[ ! -e /etc/natsume/site.toml ]] ||
  fail 'purge left server conffile /etc/natsume/site.toml behind'
[[ ! -e /etc/natsume/trust/control-ca.crt ]] ||
  fail 'purge left server conffile /etc/natsume/trust/control-ca.crt behind'
[[ ! -e /etc/natsume/trust/local-origin-ca.crt ]] ||
  fail 'purge left server conffile /etc/natsume/trust/local-origin-ca.crt behind'
[[ ! -e /usr/lib/systemd/system/natsume-server.service ]] ||
  fail 'purge left the natsume-server unit behind'

server_ip=${NATSUME_TEST_SERVER_IP:-192.0.2.10}
server_port=${NATSUME_TEST_SERVER_PORT:-8443}
reconfigure_ip=${NATSUME_TEST_RECONFIGURE_IP:-2001:db8::10}
reconfigure_port=${NATSUME_TEST_RECONFIGURE_PORT:-9443}

printf 'natsume-client natsume-client/server-ip string %s\n' "${server_ip}" |
  debconf-set-selections
printf 'natsume-client natsume-client/server-port string %s\n' "${server_port}" |
  debconf-set-selections

DEBIAN_FRONTEND=noninteractive apt-get install --yes "${client_deb}"
assert_endpoint "${server_ip}" "${server_port}"
assert_config_metadata

assert_group natsume-gateway
assert_user natsume
assert_group natsume
assert_user natsume-caddy
assert_group natsume-caddy

assert_tmpfiles_path /var/lib/natsume 'natsume:natsume 750'
assert_tmpfiles_path /var/lib/natsume/identity 'natsume:natsume 750'
assert_tmpfiles_path /var/lib/natsume/keys 'natsume:natsume 700'
assert_tmpfiles_path /run/natsume 'natsume:natsume-gateway 770'
assert_tmpfiles_path /run/natsume/gateway-tls 'natsume:natsume-gateway 750'
assert_tmpfiles_path /run/natsume/gateway-status 'natsume:natsume-gateway 750'

systemd-analyze --recursive-errors=no verify \
  /usr/lib/systemd/system/natsume-device-daemon.service \
  /usr/lib/systemd/system/natsume-privileged-helper.service \
  /usr/lib/systemd/system/natsume-caddy.service \
  /usr/lib/systemd/system/natsume-caddy.path

before_reinstall=$(sha256sum "${config}" | cut -d' ' -f1)
DEBIAN_FRONTEND=noninteractive apt-get install --reinstall --yes "${client_deb}"
after_reinstall=$(sha256sum "${config}" | cut -d' ' -f1)
[[ ${before_reinstall} == "${after_reinstall}" ]] ||
  fail 'reinstall changed the existing endpoint config'
assert_config_metadata

printf 'natsume-client natsume-client/server-ip string %s\n' "${reconfigure_ip}" |
  debconf-set-selections
printf 'natsume-client natsume-client/server-port string %s\n' "${reconfigure_port}" |
  debconf-set-selections
DEBIAN_FRONTEND=noninteractive dpkg-reconfigure natsume-client
assert_endpoint "${reconfigure_ip}" "${reconfigure_port}"
assert_config_metadata

dpkg --remove natsume-client
[[ -e ${config} ]] || fail 'remove must preserve the endpoint conffile until purge'

dpkg --purge natsume-client
[[ ! -e ${config} ]] || fail 'purge left the endpoint conffile behind'
[[ ! -e /etc/natsume/site.toml ]] ||
  fail 'purge left client conffile /etc/natsume/site.toml behind'
[[ ! -e /etc/natsume/trust/control-ca.crt ]] ||
  fail 'purge left client conffile /etc/natsume/trust/control-ca.crt behind'
[[ ! -e /etc/natsume/trust/local-origin-ca.crt ]] ||
  fail 'purge left client conffile /etc/natsume/trust/local-origin-ca.crt behind'
[[ ! -e /usr/lib/systemd/system/natsume-device-daemon.service ]] ||
  fail 'purge left the Device Daemon unit behind'
[[ ! -e /usr/lib/systemd/system/natsume-privileged-helper.service ]] ||
  fail 'purge left the Privileged Helper unit behind'
[[ ! -e /usr/lib/systemd/system/natsume-caddy.service ]] ||
  fail 'purge left the Caddy service unit behind'
[[ ! -e /usr/lib/systemd/system/natsume-caddy.path ]] ||
  fail 'purge left the Caddy path unit behind'
[[ ! -e /usr/share/dbus-1/system.d/org.natsume.Device1.conf ]] ||
  fail 'purge left the Device1 policy behind'
[[ ! -e /usr/share/dbus-1/system.d/org.natsume.Privileged1.conf ]] ||
  fail 'purge left the Privileged1 policy behind'

printf '%s\n' 'hosted-lifecycle: server install/reinstall/remove/purge and client install/reinstall/reconfigure/remove/purge passed (no reboot coverage on hosted runners)'
