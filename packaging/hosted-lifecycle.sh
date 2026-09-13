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

assert_preserved_file() {
  local path=$1 expected_hash=$2 expected_metadata=$3 actual_hash actual_metadata
  [[ -f ${path} ]] || fail "preserved file is missing: ${path}"
  actual_hash=$(sha256sum "${path}" | cut -d' ' -f1)
  [[ ${actual_hash} == "${expected_hash}" ]] ||
    fail "reinstall changed preserved file content: ${path}"
  actual_metadata=$(stat --format='%U:%G %a' "${path}")
  [[ ${actual_metadata} == "${expected_metadata}" ]] ||
    fail "preserved file ${path} metadata is ${actual_metadata}, expected ${expected_metadata}"
}

# Generate disposable public inputs here; release packages contain none.
site_files=(/etc/natsume-server/config.toml /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt)
site_inputs=$(mktemp -d)
trap 'rm -rf "${site_inputs}"' EXIT HUP INT TERM
mkdir -p "${site_inputs}/etc/natsume/trust"
for authority in control local-origin; do
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "${site_inputs}/test-ca.key" \
    -out "${site_inputs}/etc/natsume/trust/${authority}-ca.crt" \
    -days 1 -subj "/CN=Natsume Lifecycle ${authority} Test CA" >/dev/null 2>&1
done
rm -f "${site_inputs}/test-ca.key"
install -D -m 0644 packaging/server/config.example.toml "${site_inputs}/etc/natsume-server/config.toml"
install -D -m 0644 packaging/client/config.example.toml "${site_inputs}/etc/natsume/config.toml"

assert_site_inputs_absent() {
  local path
  for path in "${site_files[@]}"; do
    [[ ! -e ${path} ]] || fail "unexpected deployment-owned site input: ${path}"
  done
}

declare -A site_hashes
inject_site_inputs() {
  local path
  for path in "${site_files[@]}"; do
    install -D -o root -g root -m 0644 "${site_inputs}${path}" "${path}"
    site_hashes[${path}]=$(sha256sum "${path}" | cut -d' ' -f1)
  done
}

assert_site_inputs_preserved() {
  local path
  for path in "${site_files[@]}"; do
    assert_preserved_file "${path}" "${site_hashes[${path}]}" 'root:root 644'
  done
}

assert_site_inputs_absent
DEBIAN_FRONTEND=noninteractive apt-get install --yes "${server_deb}"
assert_site_inputs_absent
mapfile -t server_conditions < <(grep '^ConditionPathExists=' /usr/lib/systemd/system/natsume-server.service)
if systemd-analyze condition "${server_conditions[@]}"; then
  fail 'Server startup condition passed without deployment-supplied CA'
fi
inject_site_inputs
systemd-analyze condition "${server_conditions[@]}" || fail 'Server conditions failed after site injection'

assert_user natsume-server
assert_group natsume-server

assert_tmpfiles_path /var/lib/natsume-server 'natsume-server:natsume-server 750'
assert_tmpfiles_path /var/lib/natsume-server/keys 'natsume-server:natsume-server 700'
assert_tmpfiles_path /var/lib/natsume-server/backups 'natsume-server:natsume-server 750'
assert_tmpfiles_path /var/lib/natsume-server/organization-logos 'root:natsume-server 750'
assert_tmpfiles_path /var/log/natsume-server 'natsume-server:natsume-server 750'

systemd-analyze --recursive-errors=no verify \
  /usr/lib/systemd/system/natsume-server.service

logo_fixture=/var/lib/natsume-server/organization-logos/.lifecycle-preservation
printf '%s\n' 'deployment-owned logo content' >"${logo_fixture}"
chown root:natsume-server "${logo_fixture}"
chmod 0640 "${logo_fixture}"
logo_hash=$(sha256sum "${logo_fixture}" | cut -d' ' -f1)
DEBIAN_FRONTEND=noninteractive apt-get install --reinstall --yes "${server_deb}"
assert_site_inputs_preserved
assert_tmpfiles_path /var/lib/natsume-server/organization-logos 'root:natsume-server 750'
assert_preserved_file "${logo_fixture}" "${logo_hash}" 'root:natsume-server 640'

dpkg --remove natsume-server
[[ -e /etc/natsume-server/config.toml ]] ||
  fail 'remove deleted deployment-owned Server configuration /etc/natsume-server/config.toml'
assert_site_inputs_preserved

dpkg --purge natsume-server
assert_site_inputs_preserved
[[ ! -e /usr/lib/systemd/system/natsume-server.service ]] ||
  fail 'purge left the natsume-server unit behind'
# Remove only our injected fixtures so Client also starts without site inputs.
rm -f -- "${site_files[@]}"

site_files=(/etc/natsume/config.toml /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt)
assert_site_inputs_absent
DEBIAN_FRONTEND=noninteractive apt-get install --yes "${client_deb}"
assert_site_inputs_absent
mapfile -t daemon_conditions < <(grep '^ConditionPathExists=' /usr/lib/systemd/system/natsume-device-daemon.service)
if systemd-analyze condition "${daemon_conditions[@]}"; then
  fail 'Device Daemon startup condition passed without image-supplied CA'
fi

assert_group natsume-gateway
assert_user natsume
assert_group natsume
assert_user natsume-caddy
assert_group natsume-caddy

assert_tmpfiles_path /var/lib/natsume 'root:natsume-gateway 750'
assert_tmpfiles_path /var/lib/natsume/control 'natsume:natsume 750'
assert_tmpfiles_path /var/lib/natsume/identity 'natsume:natsume 750'
assert_tmpfiles_path /var/lib/natsume/keys 'natsume:natsume-gateway 2750'
assert_tmpfiles_path /run/natsume 'natsume:natsume-gateway 2770'

systemd-analyze --recursive-errors=no verify \
  /usr/lib/systemd/system/natsume-device-daemon.service \
  /usr/lib/systemd/system/natsume-privileged-helper.service \
  /usr/lib/systemd/system/natsume-caddy.service

identity_file=/var/lib/natsume/identity/identity.json
control_key_file=/var/lib/natsume/control/control-key-1.pk8
control_manifest_file=/var/lib/natsume/control/manifest.json
gateway_generation_directory=/var/lib/natsume/keys/gateway/0198f3b4-5c6d-7e8f-9abc-def012345678
gateway_key_file=${gateway_generation_directory}/key.pem
for path in "${identity_file}" "${control_key_file}" "${control_manifest_file}" "${gateway_key_file}"; do
  [[ ! -e ${path} ]] || fail "client lifecycle seed path already exists: ${path}"
done
install -d -o natsume -g natsume-gateway -m 2750 "${gateway_generation_directory}"
assert_tmpfiles_path "${gateway_generation_directory}" 'natsume:natsume-gateway 2750'
printf '%s' '{"identity":"hosted-lifecycle-fixed"}' >"${identity_file}"
printf '%s' 'hosted-lifecycle-fixed-control-key' >"${control_key_file}"
printf '%s' '{"control":"hosted-lifecycle-fixed"}' >"${control_manifest_file}"
printf '%s' 'hosted-lifecycle-fixed-gateway-key' >"${gateway_key_file}"
chown natsume:natsume \
  "${identity_file}" "${control_key_file}" "${control_manifest_file}"
chown natsume:natsume-gateway "${gateway_key_file}"
chmod 0600 \
  "${identity_file}" "${control_key_file}" "${control_manifest_file}"
chmod 0640 "${gateway_key_file}"
identity_hash_before=$(sha256sum "${identity_file}" | cut -d' ' -f1)
control_key_hash_before=$(sha256sum "${control_key_file}" | cut -d' ' -f1)
control_manifest_hash_before=$(sha256sum "${control_manifest_file}" | cut -d' ' -f1)
gateway_key_hash_before=$(sha256sum "${gateway_key_file}" | cut -d' ' -f1)
identity_metadata_before=$(stat --format='%U:%G %a' "${identity_file}")
control_key_metadata_before=$(stat --format='%U:%G %a' "${control_key_file}")
control_manifest_metadata_before=$(stat --format='%U:%G %a' "${control_manifest_file}")
gateway_key_metadata_before=$(stat --format='%U:%G %a' "${gateway_key_file}")

# Simulate deployment after the generic Client has been preinstalled.
inject_site_inputs
systemd-analyze condition "${daemon_conditions[@]}" || fail 'Device Daemon conditions failed after site injection'

before_reinstall=$(sha256sum "${config}" | cut -d' ' -f1)
DEBIAN_FRONTEND=noninteractive apt-get install --reinstall --yes "${client_deb}"
after_reinstall=$(sha256sum "${config}" | cut -d' ' -f1)
[[ ${before_reinstall} == "${after_reinstall}" ]] ||
  fail 'reinstall changed the deployment configuration'
assert_preserved_file "${identity_file}" "${identity_hash_before}" "${identity_metadata_before}"
assert_site_inputs_preserved
assert_preserved_file \
  "${control_key_file}" "${control_key_hash_before}" "${control_key_metadata_before}"
assert_preserved_file \
  "${control_manifest_file}" "${control_manifest_hash_before}" "${control_manifest_metadata_before}"
assert_preserved_file \
  "${gateway_key_file}" "${gateway_key_hash_before}" "${gateway_key_metadata_before}"
rm -f -- \
  "${identity_file}" "${control_key_file}" "${control_manifest_file}" \
  "${gateway_key_file}"
rmdir -- "${gateway_generation_directory}"

DEBIAN_FRONTEND=noninteractive dpkg-reconfigure natsume-client
assert_site_inputs_preserved

dpkg --remove natsume-client
[[ -e ${config} ]] || fail 'remove must preserve deployment configuration'

dpkg --purge natsume-client
assert_site_inputs_preserved
[[ ! -e /usr/lib/systemd/system/natsume-device-daemon.service ]] ||
  fail 'purge left the Device Daemon unit behind'
[[ ! -e /usr/lib/systemd/system/natsume-privileged-helper.service ]] ||
  fail 'purge left the Privileged Helper unit behind'
[[ ! -e /usr/lib/systemd/system/natsume-caddy.service ]] ||
  fail 'purge left the Caddy service unit behind'
[[ ! -e /usr/share/dbus-1/system.d/org.natsume.Device1.conf ]] ||
  fail 'purge left the Device1 policy behind'
[[ ! -e /usr/share/dbus-1/system.d/org.natsume.Privileged1.conf ]] ||
  fail 'purge left the Privileged1 policy behind'

printf '%s\n' 'hosted-lifecycle: server install/reinstall/remove/purge and client install/reinstall/reconfigure/remove/purge passed (no reboot coverage on hosted runners)'
