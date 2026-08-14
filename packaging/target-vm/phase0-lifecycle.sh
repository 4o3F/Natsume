#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'phase0-lifecycle: %s\n' "$*" >&2
  exit 1
}

[[ ${EUID} -eq 0 ]] || fail 'run as root on a disposable target OS VM'
[[ ${NATSUME_TARGET_VM_ACK:-} == phase0-destructive-package-lifecycle ]] ||
  fail 'set NATSUME_TARGET_VM_ACK=phase0-destructive-package-lifecycle'
[[ -d /run/systemd/system ]] || fail 'target VM must be booted with systemd'

mode=${1:-}
state_dir=/var/lib/natsume-phase0-lifecycle
state_file=${state_dir}/expected-config.sha256
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

install_package() {
  local package_path=$1
  DEBIAN_FRONTEND=noninteractive apt-get install --yes "${package_path}"
}

case ${mode} in
pre-reboot)
  current_deb=${2:-}
  previous_deb=${3:-}
  [[ -f ${current_deb} ]] || fail 'usage: phase0-lifecycle.sh pre-reboot <current.deb> [previous.deb]'

  server_ip=${NATSUME_TEST_SERVER_IP:-192.0.2.10}
  server_port=${NATSUME_TEST_SERVER_PORT:-8443}
  reconfigure_ip=${NATSUME_TEST_RECONFIGURE_IP:-2001:db8::10}
  reconfigure_port=${NATSUME_TEST_RECONFIGURE_PORT:-9443}

  # Stale state changes dpkg's install branches (conffile prompts, .dpkg-old
  # leftovers), so a dirty VM is refused instead of silently purged.
  if dpkg -s natsume-client >/dev/null 2>&1; then
    fail 'natsume-client is already installed; use a clean disposable VM'
  fi
  [[ ! -e /etc/natsume ]] ||
    fail '/etc/natsume already exists (stale residue); use a clean disposable VM'

  printf 'natsume-client natsume-client/server-ip string %s\n' "${server_ip}" |
    debconf-set-selections
  printf 'natsume-client natsume-client/server-port string %s\n' "${server_port}" |
    debconf-set-selections

  if [[ -n ${previous_deb} ]]; then
    [[ -f ${previous_deb} ]] || fail "previous package is missing: ${previous_deb}"
    install_package "${previous_deb}"
  else
    install_package "${current_deb}"
  fi
  assert_endpoint "${server_ip}" "${server_port}"
  assert_config_metadata

  before_reinstall=$(sha256sum "${config}" | cut -d' ' -f1)
  install_package "${current_deb}"
  after_reinstall=$(sha256sum "${config}" | cut -d' ' -f1)
  [[ ${before_reinstall} == "${after_reinstall}" ]] ||
    fail 'reinstall or upgrade changed the existing endpoint config'
  assert_config_metadata

  printf 'natsume-client natsume-client/server-ip string %s\n' "${reconfigure_ip}" |
    debconf-set-selections
  printf 'natsume-client natsume-client/server-port string %s\n' "${reconfigure_port}" |
    debconf-set-selections
  DEBIAN_FRONTEND=noninteractive dpkg-reconfigure natsume-client
  assert_endpoint "${reconfigure_ip}" "${reconfigure_port}"
  assert_config_metadata

  install -d -m 0700 "${state_dir}"
  sha256sum "${config}" | cut -d' ' -f1 >"${state_file}"
  chmod 0600 "${state_file}"
  printf '%s\n' 'phase0-lifecycle: pre-reboot checks passed; reboot, then run post-reboot'
  ;;
post-reboot)
  [[ -r ${state_file} ]] || fail 'pre-reboot state is missing'
  expected=$(<"${state_file}")
  actual=$(sha256sum "${config}" | cut -d' ' -f1)
  [[ ${actual} == "${expected}" ]] || fail 'endpoint config changed across reboot'
  assert_config_metadata

  dpkg --remove natsume-client
  [[ -e ${config} ]] || fail 'remove must preserve the conffile until purge'
  dpkg --purge natsume-client
  [[ ! -e ${config} ]] || fail 'purge left the endpoint conffile behind'
  [[ ! -e /usr/lib/systemd/system/natsume-device-daemon.service ]] ||
    fail 'purge left the Device Daemon unit behind'
  [[ ! -e /usr/share/dbus-1/system.d/org.natsume.Device1.conf ]] ||
    fail 'purge left the Device1 policy behind'

  rm -rf "${state_dir}"
  printf '%s\n' 'phase0-lifecycle: install/reinstall/upgrade/reconfigure/reboot/remove/purge passed'
  ;;
*)
  fail 'usage: phase0-lifecycle.sh pre-reboot <current.deb> [previous.deb] | post-reboot'
  ;;
esac
