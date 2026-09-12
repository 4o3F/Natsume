#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repository_root}"

fail() {
  printf 'package-smoke: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

download() {
  curl --fail --silent --show-error --location \
    --retry 3 --retry-all-errors --retry-delay 2 \
    "$1" --output "$2"
}

for command in cargo cp curl cut dpkg-deb envsubst grep node pnpm python3 readelf sed sha256sum shellcheck systemctl systemd-analyze tar; do
  require_command "${command}"
done

python3 packaging/check-image-inputs.py
python3 packaging/check-maintainer-scripts.py

session_kiosk_source='packaging/client/rootfs/usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf'
test -f "${session_kiosk_source}" || fail 'GNOME Kiosk Agent drop-in is missing'
grep -Fxq 'ExecStart=/usr/bin/natsume-session-agent run' "${session_kiosk_source}" ||
  fail 'GNOME Kiosk Agent drop-in has an unexpected command'
grep -Fxq 'ConditionUser=waiting' "${session_kiosk_source}" ||
  fail 'GNOME Kiosk Agent drop-in must restrict the waiting user'
test ! -e packaging/client/rootfs/etc/xdg/autostart/org.natsume.SessionAgent.desktop ||
  fail 'Session Agent must have only the GNOME Kiosk startup owner'

work_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/natsume-package-smoke.XXXXXX")"
trap 'rm -rf "${work_root}"' EXIT HUP INT TERM

tool_root="${work_root}/tools"
extract_root="${work_root}/extract"
output_root="${NATSUME_PACKAGE_OUTPUT:-${repository_root}/dist/packages/ci}"
mkdir -p "${tool_root}" "${extract_root}" "${output_root}"

caddy_version="$(tr -d '[:space:]' <packaging/client/caddy.version)"
nfpm_version="$(tr -d '[:space:]' <packaging/nfpm.version)"
caddy_archive="caddy_${caddy_version}_linux_amd64.tar.gz"
nfpm_archive="nfpm_${nfpm_version}_Linux_x86_64.tar.gz"

download \
  "https://github.com/caddyserver/caddy/releases/download/v${caddy_version}/${caddy_archive}" \
  "${tool_root}/${caddy_archive}"
download \
  "https://github.com/goreleaser/nfpm/releases/download/v${nfpm_version}/${nfpm_archive}" \
  "${tool_root}/${nfpm_archive}"

(
  cd "${tool_root}"
  sha256sum --check "${repository_root}/packaging/client/caddy.archive.sha256"
  sha256sum --check "${repository_root}/packaging/nfpm.sha256"
)

tar -xzf "${tool_root}/${caddy_archive}" -C "${tool_root}" caddy
tar -xzf "${tool_root}/${nfpm_archive}" -C "${tool_root}" nfpm
caddy_binary="${tool_root}/caddy"
nfpm_binary="${tool_root}/nfpm"
test -x "${caddy_binary}" || fail 'Caddy archive did not contain an executable caddy binary'
test -x "${nfpm_binary}" || fail 'nFPM archive did not contain an executable nfpm binary'

expected_caddy_sha="$(cut -d' ' -f1 packaging/client/caddy.sha256)"
actual_caddy_sha="$(sha256sum "${caddy_binary}" | cut -d' ' -f1)"
[[ ${actual_caddy_sha} == "${expected_caddy_sha}" ]] ||
  fail 'extracted Caddy binary SHA-256 does not match caddy.sha256'

"${caddy_binary}" version | grep -Fq "v${caddy_version}" ||
  fail 'Caddy binary version does not match caddy.version'
"${nfpm_binary}" --version | grep -Fq "GitVersion:    ${nfpm_version}" ||
  fail 'nFPM binary version does not match nfpm.version'

module_list="${work_root}/caddy-modules.txt"
"${caddy_binary}" list-modules >"${module_list}"
while IFS= read -r module || [[ -n ${module} ]]; do
  [[ -z ${module} || ${module} == \#* ]] && continue
  grep -Fxq "${module}" "${module_list}" ||
    fail "Caddy is missing required module: ${module}"
done <packaging/client/caddy.modules

if "${caddy_binary}" list-modules --skip-standard | grep -Eq '^[a-z0-9]'; then
  fail 'Caddy binary contains a non-standard module'
fi
"${caddy_binary}" fmt --diff \
  packaging/client/rootfs/etc/natsume/caddy/bootstrap.caddyfile \
  >/dev/null
"${caddy_binary}" adapt \
  --adapter caddyfile \
  --config packaging/client/rootfs/etc/natsume/caddy/bootstrap.caddyfile \
  >/dev/null

CADDY_BIN="${caddy_binary}" cargo test --locked -p natsume-device-daemon \
  reconcile::caddy::tests::packaged_caddy_preserves_literal_usernames -- --ignored --exact

# Build only the binaries that enter the packages, in an isolated target directory.
production_target="${work_root}/cargo-target-production"
production_release="${production_target}/release"
CARGO_TARGET_DIR="${production_target}" cargo build \
  --release \
  --locked \
  -p natsume-device-daemon \
  -p natsume-privileged-helper \
  -p natsume-session-agent \
  -p natsume-server
pnpm --filter @natsume/web build

export VERSION="${VERSION:-2.0.4~ci1}"
export ARCH="${ARCH:-amd64}"
export RUST_RELEASE_DIR="${production_release}"
export CADDY_BIN="${caddy_binary}"
# Both packages must build without deployment-specific inputs.
unset SITE_CONFIG CONTROL_CA_CERT LOCAL_ORIGIN_CA_CERT

# envsubst takes literal variable names, not their values.
# shellcheck disable=SC2016
server_variables='${ARCH} ${VERSION} ${RUST_RELEASE_DIR}'
# shellcheck disable=SC2016
client_variables='${ARCH} ${VERSION} ${RUST_RELEASE_DIR} ${CADDY_BIN}'
server_config="${work_root}/server.nfpm.yaml"
client_config="${work_root}/client.nfpm.yaml"
envsubst "${server_variables}" <packaging/server/nfpm.yaml >"${server_config}"
envsubst "${client_variables}" <packaging/client/nfpm.yaml >"${client_config}"
if grep -Eq '\$\{[A-Z_]+\}' "${server_config}" "${client_config}"; then
  fail 'rendered nFPM configuration contains an unresolved environment variable'
fi

"${nfpm_binary}" package \
  --packager deb \
  --config "${server_config}" \
  --target "${output_root}/"
"${nfpm_binary}" package \
  --packager deb \
  --config "${client_config}" \
  --target "${output_root}/"

server_deb="${output_root}/natsume-server_${VERSION}_${ARCH}.deb"
client_deb="${output_root}/natsume-client_${VERSION}_${ARCH}.deb"
test -f "${server_deb}" || fail "server Deb was not produced at ${server_deb}"
test -f "${client_deb}" || fail "client Deb was not produced at ${client_deb}"
python3 packaging/check-image-inputs.py --deb "${client_deb}"

dpkg-deb --info "${server_deb}" >/dev/null
dpkg-deb --info "${client_deb}" >/dev/null
dpkg-deb --contents "${server_deb}" >"${work_root}/server.contents"
dpkg-deb --contents "${client_deb}" >"${work_root}/client.contents"

require_package_path() {
  local listing="$1"
  local package_path="$2"
  local line

  while IFS= read -r line; do
    if [[ ${line##* } == ".${package_path}" ]]; then
      return 0
    fi
  done <"${listing}"

  fail "package is missing required path: ${package_path}"
}

for path in \
  /usr/bin/natsume-server \
  /usr/share/doc/natsume-server/config.example.toml \
  /usr/lib/systemd/system/natsume-server.service \
  /usr/share/natsume-server/web/index.html; do
  require_package_path "${work_root}/server.contents" "${path}"
done

grep -E '\./usr/share/natsume-server/web/assets/[^/]+$' \
  "${work_root}/server.contents" >/dev/null ||
  fail 'server package web asset directory is empty'

for path in \
  /usr/bin/natsume-device-daemon \
  /usr/lib/natsume/natsume-privileged-helper \
  /usr/bin/natsume-session-agent \
  /usr/lib/natsume/caddy \
  /usr/lib/systemd/system/natsume-device-daemon.service \
  /usr/lib/systemd/system/natsume-privileged-helper.service \
  /usr/lib/systemd/system/natsume-caddy.service \
  /usr/share/doc/natsume-client/config.example.toml \
  /usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf \
  /usr/share/dbus-1/system.d/org.natsume.Device1.conf \
  /usr/share/dbus-1/system.d/org.natsume.Privileged1.conf; do
  require_package_path "${work_root}/client.contents" "${path}"
done

if grep -Fq 'org.natsume.SessionAgent.desktop' "${work_root}/client.contents"; then
  fail 'client package unexpectedly contains a second Agent startup entry'
fi
if grep -Fiq 'identity-guard' "${work_root}/server.contents" "${work_root}/client.contents"; then
  fail 'Identity Guard path is present in a package'
fi

grep -E '^-rwxr-xr-x .*\./usr/bin/natsume-device-daemon$' "${work_root}/client.contents" >/dev/null ||
  fail 'device daemon package mode is not 0755'
grep -E '^-rwxr-xr-x .*\./usr/lib/natsume/caddy$' "${work_root}/client.contents" >/dev/null ||
  fail 'Caddy package mode is not 0755'
grep -E '^-rw-r--r-- .*\./usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf$' \
  "${work_root}/client.contents" >/dev/null ||
  fail 'GNOME Kiosk Agent drop-in package mode is not 0644'

# The Session Agent links the Slint/Skia closure; its direct ELF NEEDED set is
# frozen in session-agent.needed and every non-baseline library must be a
# declared Deb dependency, or the binary dies in ld.so before main.
readelf -d "${production_release}/natsume-session-agent" |
  sed -nE 's/.*\(NEEDED\).*\[(.*)\].*/\1/p' | sort >"${work_root}/session-agent.needed.actual"
diff -u packaging/client/session-agent.needed "${work_root}/session-agent.needed.actual" ||
  fail 'Session Agent ELF NEEDED set drifted from packaging/client/session-agent.needed'
client_depends="$(dpkg-deb --field "${client_deb}" Depends)"
for package in libfontconfig1 libfreetype6 libstdc++6 libpam-modules util-linux; do
  printf '%s\n' "${client_depends}" | grep -Fq "${package}" ||
    fail "client package does not declare required dependency: ${package}"
done

shellcheck -x \
  packaging/client/scripts/postinstall.sh \
  packaging/client/scripts/preremove.sh \
  packaging/client/scripts/postremove.sh \
  packaging/hosted-lifecycle.sh \
  packaging/server/scripts/postinstall.sh \
  packaging/image/fragments/gdm/PostLogin.sh

server_control="${work_root}/server-control"
dpkg-deb --control "${server_deb}" "${server_control}"
client_control="${work_root}/client-control"
dpkg-deb --control "${client_deb}" "${client_control}"
cmp packaging/server/scripts/postinstall.sh "${server_control}/postinst" ||
  fail 'Server postinst differs from the checked deployment contract'
cmp packaging/client/scripts/postinstall.sh "${client_control}/postinst" ||
  fail 'Client postinst differs from the checked deployment contract'
if [[ -e ${client_control}/config || -e ${client_control}/templates ]]; then
  fail 'Client package still contains the retired debconf interface'
fi
cmp packaging/client/scripts/preremove.sh "${client_control}/prerm" ||
  fail 'Client prerm does not carry the maintenance removal guard'
cmp packaging/client/scripts/postremove.sh "${client_control}/postrm" ||
  fail 'Client postrm does not carry removal and purge cleanup'
test -x "${client_control}/prerm" || fail 'Client prerm is not executable'
test -x "${client_control}/postrm" || fail 'Client postrm is not executable'
if grep -Eq 'systemd-(sysusers|tmpfiles).*[|][|][[:space:]]*true' \
  packaging/client/scripts/postinstall.sh packaging/server/scripts/postinstall.sh; then
  fail 'required sysusers/tmpfiles failures are suppressed'
fi

dpkg-deb --extract "${server_deb}" "${extract_root}/server"
dpkg-deb --extract "${client_deb}" "${extract_root}/client"

client_caddyfile="${extract_root}/client/etc/natsume/caddy/bootstrap.caddyfile"
client_caddy_unit="${extract_root}/client/usr/lib/systemd/system/natsume-caddy.service"
client_daemon_unit="${extract_root}/client/usr/lib/systemd/system/natsume-device-daemon.service"
client_helper_unit="${extract_root}/client/usr/lib/systemd/system/natsume-privileged-helper.service"
server_unit="${extract_root}/server/usr/lib/systemd/system/natsume-server.service"
for path in /etc/natsume/config.toml /etc/natsume-server/config.toml /etc/natsume/site.toml /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt; do
  for package in server client; do
    test ! -e "${extract_root}/${package}${path}" ||
      fail "${package} package contains deployment-supplied site input: ${path}"
    if [[ -f ${work_root}/${package}-control/conffiles ]] &&
      grep -Fxq "${path}" "${work_root}/${package}-control/conffiles"; then
      fail "${package} package owns deployment-supplied site input: ${path}"
    fi
  done
done
for package in server client; do
  unit=${server_unit}
  config=/etc/natsume-server/config.toml
  if [[ ${package} == client ]]; then
    unit=${client_daemon_unit}
    config=/etc/natsume/config.toml
  fi
  for path in "${config}" /etc/natsume/trust/control-ca.crt /etc/natsume/trust/local-origin-ca.crt; do
    grep -Fxq "ConditionPathExists=${path}" "${unit}" ||
      fail "${package} can start without deployment input: ${path}"
  done
done
client_display_dropin="${extract_root}/client/usr/lib/systemd/system/display-manager.service.d/50-natsume-home.conf"
client_tmpfiles="${extract_root}/client/usr/lib/tmpfiles.d/natsume.conf"

grep -Fq 'admin unix//run/natsume/caddy-admin.sock|0660' "${client_caddyfile}" ||
  fail 'packaged bootstrap Caddyfile does not expose the group-writable local admin socket'
if grep -Eq '^[[:space:]]*(https?://|tls |reverse_proxy)' "${client_caddyfile}"; then
  fail 'packaged bootstrap Caddyfile must not expose a listener or upstream'
fi
grep -Fxq 'Group=natsume-gateway' "${client_caddy_unit}" ||
  fail 'packaged Caddy service does not own its admin socket through natsume-gateway'
grep -Fxq 'ReadWritePaths=/run/natsume' "${client_caddy_unit}" ||
  fail 'packaged Caddy service cannot create its runtime artifacts'
grep -Fxq 'ExecStart=/usr/bin/natsume-device-daemon run' "${client_daemon_unit}" ||
  fail 'packaged Device Daemon service does not use the explicit run command'
grep -Fxq 'ExecStopPost=+/usr/bin/systemctl kill --kill-whom=all --signal=SIGKILL natsume-caddy.service' \
  "${client_daemon_unit}" || fail 'packaged Daemon stop does not hard-stop the loaded Caddy config'
if grep -Eq '^(ProtectSystem|ProtectHome)=' "${client_helper_unit}"; then
  fail 'packaged privileged helper would isolate the managed Home mount'
fi
grep -Fxq 'PrivateNetwork=yes' "${client_helper_unit}" ||
  fail 'packaged privileged helper must retain its private network namespace'
grep -Fxq 'PrivateMounts=no' "${client_helper_unit}" ||
  fail 'packaged privileged helper must explicitly share the host mount namespace'
grep -Fxq 'OpenFile=/proc/1/ns/mnt:host-mount-namespace:read-only' "${client_helper_unit}" ||
  fail 'packaged privileged helper must receive the host mount namespace descriptor'
grep -Fxq 'RestrictAddressFamilies=AF_UNIX' "${client_helper_unit}" ||
  fail 'packaged privileged helper must restrict sockets to AF_UNIX'
grep -Fxq 'CapabilityBoundingSet=CAP_CHOWN CAP_DAC_OVERRIDE CAP_FOWNER CAP_KILL CAP_SYS_ADMIN CAP_SETUID CAP_SETGID' \
  "${client_helper_unit}" || fail 'packaged privileged helper has an unexpected capability set'
for directive in 'Type=dbus' 'BusName=org.natsume.Privileged1' \
  'ExecStopPost=/usr/lib/natsume/natsume-privileged-helper close-admission' \
  'ExecStart=/usr/lib/natsume/natsume-privileged-helper serve' 'AmbientCapabilities=CAP_SETUID' \
  'Restart=on-failure' 'RestartSec=2s' 'StartLimitIntervalSec=60s' 'StartLimitBurst=5'; do
  grep -Fxq "${directive}" "${client_helper_unit}" ||
    fail "packaged privileged helper is missing ${directive}"
done
client_prepare_unit="${extract_root}/client/usr/lib/systemd/system/natsume-session-prepare@.service"
for directive in 'Type=oneshot' 'User=root' 'Restart=no' 'TimeoutStartSec=45s' \
  'TimeoutStopSec=5s' 'KillMode=control-group' 'AmbientCapabilities=CAP_SETUID' \
  'CapabilityBoundingSet=CAP_DAC_OVERRIDE CAP_KILL CAP_SETUID CAP_SETGID' \
  'ExecStart=/usr/lib/natsume/natsume-privileged-helper prepare-session %i'; do
  grep -Fxq "${directive}" "${client_prepare_unit}" || fail "fixed GDM entry lacks ${directive}"
done
for role in waiting contest; do
  pam_stack="${extract_root}/client/etc/pam.d/gdm-${role}"
  account=${role}
  [[ ${role} != contest ]] || account=teams
  grep -Fxq "auth requisite pam_succeed_if.so user = ${account} quiet" "${pam_stack}" ||
    fail "GDM ${role} stack does not restrict its user"
  grep -Fxq '@include gdm-autologin' "${pam_stack}" || fail 'fixed GDM stack must include the vendor stack'
done
grep -Fxq '@include natsume-contest-admission' "${extract_root}/client/etc/pam.d/gdm-contest" ||
  fail 'fixed contest entry must carry its own admission gate'
for phase in auth account session; do
  grep -Fxq "${phase} requisite pam_exec.so quiet /usr/lib/natsume/natsume-privileged-helper pam-gate" \
    "${extract_root}/client/etc/pam.d/natsume-contest-admission" || fail "PAM gate lacks ${phase}"
done
for vendor in gdm-autologin gdm-password; do
  test ! -e "${extract_root}/client/etc/pam.d/${vendor}" || fail 'Client must not own vendor GDM PAM files'
done
test ! -e "${client_display_dropin}" || fail 'global GDM Home interlock must be removed'
grep -Fxq 'd /run/natsume-privileged 0700 root root -' "${client_tmpfiles}" ||
  fail 'packaged Home login permit directory is not root-only'
grep -Fxq 'd /run/natsume-privileged/contest-workers 0700 root root -' "${client_tmpfiles}" ||
  fail 'packaged PAM worker directory is not root-only'
grep -Fxq 'd /run/natsume 2770 natsume natsume-gateway -' "${client_tmpfiles}" ||
  fail 'packaged Caddy runtime directory cannot inherit the gateway group'
grep -Fxq 'd /var/lib/natsume 0750 root natsume-gateway -' "${client_tmpfiles}" ||
  fail 'packaged Device state root is not protected from the Daemon user'
grep -Fxq 'd /var/lib/natsume/state 0700 natsume natsume -' "${client_tmpfiles}" ||
  fail 'packaged Device state directory is not fixed before Daemon startup'
grep -Fxq 'd /var/lib/natsume-privileged/home-reset 0700 root root -' "${client_tmpfiles}" ||
  fail 'packaged Home reset state is not rooted outside Daemon-owned storage'

# The target base system supplies systemctl; make it visible to --root verification.
mkdir -p "${extract_root}/client/usr/bin"
cp "$(command -v systemctl)" "${extract_root}/client/usr/bin/systemctl"

grep -Fxq 'LimitNOFILE=4096' "${extract_root}/server/usr/lib/systemd/system/natsume-server.service" ||
  fail 'packaged Server FD limit must leave room above its 1024 connection budget'

systemd-analyze --recursive-errors=no --root="${extract_root}/server" verify \
  /usr/lib/systemd/system/natsume-server.service
systemd-analyze --recursive-errors=no --root="${extract_root}/client" verify \
  /usr/lib/systemd/system/natsume-device-daemon.service \
  /usr/lib/systemd/system/natsume-privileged-helper.service \
  /usr/lib/systemd/system/natsume-session-prepare@contest.service \
  /usr/lib/systemd/system/natsume-caddy.service

session_kiosk="${extract_root}/client/usr/lib/systemd/user/org.gnome.Kiosk.Script.service.d/50-natsume.conf"
cmp "${session_kiosk_source}" "${session_kiosk}" ||
  fail 'packaged GNOME Kiosk Agent configuration differs from its source'

python3 - "${extract_root}/client" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

root = Path(sys.argv[1])
policies = sorted((root / "usr/share/dbus-1/system.d").glob("*.conf"))
if not policies:
    raise SystemExit("client package contains no D-Bus policy files")
for policy in policies:
    ET.parse(policy)
PY

printf 'package-smoke: ok server=%s client=%s\n' "${server_deb}" "${client_deb}"
