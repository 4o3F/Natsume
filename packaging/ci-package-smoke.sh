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

for command in cargo curl cut dpkg-deb envsubst grep node openssl pnpm python3 readelf sed sha256sum shellcheck systemd-analyze tar; do
  require_command "${command}"
done

session_autostart_source='packaging/client/rootfs/etc/xdg/autostart/org.natsume.SessionAgent.desktop'
session_user_unit_source='packaging/client/rootfs/usr/lib/systemd/user/natsume-session-agent.service'
test -f "${session_autostart_source}" || fail 'XDG Autostart entry is missing from the Client rootfs'
test ! -e "${session_user_unit_source}" || fail 'Session Agent systemd user unit must not be packaged'
grep -Fxq 'Exec=/usr/bin/natsume-session-agent --autostart' "${session_autostart_source}" ||
  fail 'XDG Autostart entry has an unexpected Exec'
if grep -Eq '^(OnlyShowIn|NotShowIn)=' "${session_autostart_source}"; then
  fail 'XDG Autostart entry must remain desktop-neutral'
fi

work_root="$(mktemp -d "${RUNNER_TEMP:-/tmp}/natsume-package-smoke.XXXXXX")"
trap 'rm -rf "${work_root}"' EXIT HUP INT TERM

tool_root="${work_root}/tools"
input_root="${work_root}/inputs"
extract_root="${work_root}/extract"
output_root="${NATSUME_PACKAGE_OUTPUT:-${repository_root}/dist/packages/ci}"
mkdir -p "${tool_root}" "${input_root}" "${extract_root}" "${output_root}"

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

cat >"${input_root}/site.toml" <<'EOF'
schema_version = 1
fleet_namespace_uuid = "00000000-0000-4000-8000-000000000001"
gateway_hostname = "gateway.contest.example"
gateway_not_after = "2030-01-01T00:00:00Z"
contest_end = "2029-12-31T00:00:00Z"

[trust]
control_root_sha256 = "0000000000000000000000000000000000000000000000000000000000000001"
local_origin_root_sha256 = "0000000000000000000000000000000000000000000000000000000000000002"
EOF

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${input_root}/control-ca.key" \
  -out "${input_root}/control-ca.crt" \
  -days 1 \
  -subj '/CN=Natsume CI Control Test CA' \
  >/dev/null 2>&1
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${input_root}/local-origin-ca.key" \
  -out "${input_root}/local-origin-ca.crt" \
  -days 1 \
  -subj '/CN=Natsume CI Local Origin Test CA' \
  >/dev/null 2>&1
rm -f "${input_root}/control-ca.key" "${input_root}/local-origin-ca.key"

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
canonical_endpoint="$("${production_release}/natsume-device-daemon" --print-canonical-endpoint '2001:0db8:0:0:0:0:0:10' '8443')"
[[ ${canonical_endpoint} == '2001:db8::10 8443' ]] ||
  fail 'Device Daemon did not emit the canonical IPv6 endpoint'
if "${production_release}/natsume-device-daemon" --print-canonical-endpoint 'server.example' '8443' >/dev/null 2>&1; then
  fail 'Device Daemon accepted a hostname as an install endpoint'
fi
pnpm --filter @natsume/web build

export VERSION="${VERSION:-2.0.0~ci1}"
export ARCH="${ARCH:-amd64}"
export RUST_RELEASE_DIR="${production_release}"
export CADDY_BIN="${caddy_binary}"
export SITE_CONFIG="${input_root}/site.toml"
export CONTROL_CA_CERT="${input_root}/control-ca.crt"
export LOCAL_ORIGIN_CA_CERT="${input_root}/local-origin-ca.crt"

nfpm_variables='${ARCH} ${VERSION} ${RUST_RELEASE_DIR} ${CADDY_BIN} ${SITE_CONFIG} ${CONTROL_CA_CERT} ${LOCAL_ORIGIN_CA_CERT}'
server_config="${work_root}/server.nfpm.yaml"
client_config="${work_root}/client.nfpm.yaml"
envsubst "${nfpm_variables}" <packaging/server/nfpm.yaml >"${server_config}"
envsubst "${nfpm_variables}" <packaging/client/nfpm.yaml >"${client_config}"
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
  /usr/lib/systemd/system/natsume-caddy.path \
  /etc/natsume/config.toml \
  /etc/xdg/autostart/org.natsume.SessionAgent.desktop \
  /usr/share/dbus-1/system.d/org.natsume.Device1.conf \
  /usr/share/dbus-1/system.d/org.natsume.Privileged1.conf \
  /usr/share/natsume/gateway-status/index.html \
  /usr/share/natsume/gateway-status/status.css \
  /usr/share/natsume/gateway-status/status.js \
  /usr/share/natsume/gateway-status/icons.svg; do
  require_package_path "${work_root}/client.contents" "${path}"
done

if grep -Fq 'natsume-session-agent.service' "${work_root}/client.contents"; then
  fail 'client package unexpectedly contains a Session Agent user unit'
fi
if grep -Fiq 'identity-guard' "${work_root}/server.contents" "${work_root}/client.contents"; then
  fail 'Identity Guard path is present in a package'
fi

grep -E '^-rwxr-xr-x .*\./usr/bin/natsume-device-daemon$' "${work_root}/client.contents" >/dev/null ||
  fail 'device daemon package mode is not 0755'
grep -E '^-rwxr-xr-x .*\./usr/lib/natsume/caddy$' "${work_root}/client.contents" >/dev/null ||
  fail 'Caddy package mode is not 0755'
grep -E '^-rw-r--r-- .*\./etc/xdg/autostart/org.natsume.SessionAgent.desktop$' \
  "${work_root}/client.contents" >/dev/null ||
  fail 'XDG Autostart entry package mode is not 0644'

# The Session Agent links the Slint/Skia closure; its direct ELF NEEDED set is
# frozen in session-agent.needed and every non-baseline library must be a
# declared Deb dependency, or the binary dies in ld.so before main.
readelf -d "${production_release}/natsume-session-agent" |
  sed -nE 's/.*\(NEEDED\).*\[(.*)\].*/\1/p' | sort >"${work_root}/session-agent.needed.actual"
diff -u packaging/client/session-agent.needed "${work_root}/session-agent.needed.actual" ||
  fail 'Session Agent ELF NEEDED set drifted from packaging/client/session-agent.needed'
client_depends="$(dpkg-deb --field "${client_deb}" Depends)"
for package in libfontconfig1 libfreetype6 libstdc++6; do
  printf '%s\n' "${client_depends}" | grep -Fq "${package}" ||
    fail "client package does not declare required dependency: ${package}"
done

shellcheck -x \
  packaging/client/debconf/config \
  packaging/client/scripts/postinstall.sh \
  packaging/hosted-lifecycle.sh \
  packaging/server/scripts/postinstall.sh \
  packaging/target-vm/phase0-lifecycle.sh

client_control="${work_root}/client-control"
dpkg-deb --control "${client_deb}" "${client_control}"
grep -Fxq '/etc/natsume/config.toml' "${client_control}/conffiles" ||
  fail 'endpoint config is not registered as a Debian conffile'
if grep -Eq 'systemd-(sysusers|tmpfiles).*[|][|][[:space:]]*true' \
  packaging/client/scripts/postinstall.sh packaging/server/scripts/postinstall.sh; then
  fail 'required sysusers/tmpfiles failures are suppressed'
fi

dpkg-deb --extract "${server_deb}" "${extract_root}/server"
dpkg-deb --extract "${client_deb}" "${extract_root}/client"

client_caddyfile="${extract_root}/client/etc/natsume/caddy/bootstrap.caddyfile"
client_config_placeholder="${extract_root}/client/etc/natsume/config.toml"
client_status_root="${extract_root}/client/usr/share/natsume/gateway-status"
grep -Fxq '# Natsume endpoint is written by postinstall after debconf validation.' \
  "${client_config_placeholder}" ||
  fail 'packaged endpoint conffile is not the fail-closed placeholder'
if grep -Eq '^[[:space:]]*(ip|port)[[:space:]]*=' "${client_config_placeholder}"; then
  fail 'packaged endpoint placeholder contains an unvalidated endpoint'
fi
node --check "${client_status_root}/status.js"
grep -Fq 'Content-Security-Policy' "${client_caddyfile}" ||
  fail 'packaged bootstrap Caddyfile does not set Content-Security-Policy'
grep -Fq 'status 503' "${client_caddyfile}" ||
  fail 'packaged bootstrap Caddyfile does not force the bootstrap response to 503'
if grep -RFiq -- 'session_locked' "${client_caddyfile}" "${client_status_root}"; then
  fail 'session_locked must not appear in packaged Gateway status surfaces'
fi

systemd-analyze --recursive-errors=no --root="${extract_root}/server" verify \
  /usr/lib/systemd/system/natsume-server.service
systemd-analyze --recursive-errors=no --root="${extract_root}/client" verify \
  /usr/lib/systemd/system/natsume-device-daemon.service \
  /usr/lib/systemd/system/natsume-privileged-helper.service \
  /usr/lib/systemd/system/natsume-caddy.service \
  /usr/lib/systemd/system/natsume-caddy.path

session_autostart="${extract_root}/client/etc/xdg/autostart/org.natsume.SessionAgent.desktop"
grep -Fxq 'Exec=/usr/bin/natsume-session-agent --autostart' "${session_autostart}" ||
  fail 'packaged XDG Autostart entry has an unexpected Exec'
if grep -Eq '^(OnlyShowIn|NotShowIn)=' "${session_autostart}"; then
  fail 'packaged XDG Autostart entry must remain desktop-neutral'
fi

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
