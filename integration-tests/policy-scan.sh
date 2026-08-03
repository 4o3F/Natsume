#!/usr/bin/env bash
set -euo pipefail
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repository_root}"

fail() { printf 'policy-scan: %s\n' "$*" >&2; exit 1; }
reject_matches() {
  local description="$1" pattern="$2"; shift 2
  local matches status
  if matches=$(grep -RInE --exclude-dir=node_modules --exclude-dir=target --exclude-dir=dist \
      --exclude-dir=playwright-report --exclude-dir=test-results --exclude=policy-scan.sh \
      -- "${pattern}" "$@"); then
    printf '%s\n' "${matches}" >&2; fail "${description}"
  else
    status=$?; [[ ${status} -eq 1 ]] || fail "scanner error while checking ${description}"
  fi
}

# Same as reject_matches, but drops lines matching an allow pattern first. Used where the
# forbidden token is a short word that also appears in a legitimate unrelated construct.
reject_matches_except() {
  local description="$1" pattern="$2" allow="$3"; shift 3
  local raw status matches
  raw=$(grep -RInE --exclude-dir=node_modules --exclude-dir=target --exclude-dir=dist \
      --exclude-dir=playwright-report --exclude-dir=test-results --exclude=policy-scan.sh \
      -- "${pattern}" "$@") || {
    status=$?; [[ ${status} -eq 1 ]] || fail "scanner error while checking ${description}"
    return 0
  }
  matches=$(printf '%s\n' "${raw}" | grep -vE -- "${allow}") || {
    status=$?; [[ ${status} -eq 1 ]] || fail "scanner error while filtering ${description}"
    return 0
  }
  [[ -z ${matches} ]] || { printf '%s\n' "${matches}" >&2; fail "${description}"; }
}

private_key_pattern='BEGIN ([A-Z ]+ )?PRIVATE KEY'
printf '%s\n' '-----BEGIN PRIVATE KEY-----' | grep -Eq "${private_key_pattern}" || fail 'private-key canary was not detected'

while IFS= read -r usage; do
  reference="${usage##*@}"
  [[ ${reference} =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "GitHub Action is not pinned to an exact release version: ${usage}"
done < <(grep -RhoE 'uses:[[:space:]]+[^[:space:]#]+@[^[:space:]#]+' .github/workflows | sed -E 's/^uses:[[:space:]]+//')

reject_matches 'placeholder CI command remains' 'Implementation requirement|run:[[:space:]]*echo[[:space:]].*(placeholder|Pin actions|Run frozen)' .github/workflows
reject_matches 'private-key material is present in first-party paths' "${private_key_pattern}" .github server client crates integration-tests packaging web docs
reject_matches 'credential-shaped token is present' 'AKIA[0-9A-Z]{16}|gh[pousr]_[0-9A-Za-z]{20,}' .github server client crates integration-tests packaging web docs
reject_matches 'systemd credential directive is present' 'LoadCredential=|SetCredential=|ImportCredential=' packaging/client/rootfs packaging/server/rootfs
reject_matches 'Identity Guard product surface is present' 'natsume-identity-guard|identity_guard|IdentityGuard' server client crates integration-tests packaging
reject_matches 'maintainer script downloads at install time' '(^|[^[:alnum:]_])(curl|wget|aria2c)([^[:alnum:]_]|$)|https?://|cargo[[:space:]]+install|pnpm[[:space:]]+(add|install)|npm[[:space:]]+install|go[[:space:]]+install' packaging/client/scripts packaging/server/scripts packaging/client/debconf
reject_matches 'first-party Rust code depends on anyhow or thiserror' '(^|[^[:alnum:]_])(anyhow|thiserror)([^[:alnum:]_]|$)' server client crates integration-tests
reject_matches 'first-party Rust code parses Display text for behavior' '\.to_string\(\)[[:space:]]*\.(contains|starts_with|ends_with)|format!\([^;]*\)[[:space:]]*\.(contains|starts_with|ends_with)' server client crates
reject_matches 'quinn dependency or source usage is present' '(^|[^[:alnum:]_])quinn([^[:alnum:]_]|$)' Cargo.toml server client crates integration-tests packaging web
reject_matches 'QUIC transport surface is present' '([Qq][Uu][Ii][Cc][[:space:]-]+(transport|control|gateway|session|client|listener|endpoint|over)|[Qq][Uu][Ii][Cc][[:space:]]*=|Device[[:space:]]+[Qq][Uu][Ii][Cc]|mTLS[[:space:]-]+[Qq][Uu][Ii][Cc])' server client crates integration-tests packaging web
reject_matches 'custom length-prefix framing is present' 'encode_frame|decode_frame|length[-_[:space:]]*(prefix|delimited)' server client crates integration-tests packaging web
reject_matches 'mTLS client-certificate verifier is present' 'WebPkiClientVerifier|ClientCertVerifier|client_auth' server client crates integration-tests packaging web
reject_matches 'spreadsheet adapter surface is present' '(^|[^[:alnum:]_])(calamine|umya-spreadsheet|exceljs|sheetjs|xlsx-js-style)([^[:alnum:]_]|$)' server client crates integration-tests web package.json Cargo.toml
reject_matches_except 'generic EXEC command capability is present' '(^|[^A-Za-z])EXEC([^A-Za-z]|$)|(^|[^A-Za-z0-9_])exec([^A-Za-z0-9_]|$)|::[[:space:]]*Exec|(^|[^A-Za-z0-9_])Exec[[:space:]]*[,({]' '(pnpm|npm|npx|yarn|cargo)[[:space:]]+(--filter[[:space:]]+[^[:space:]]+[[:space:]]+)?exec[[:space:]]' server client crates integration-tests packaging web
reject_matches 'generic RUN_SHELL command capability is present' '(^|[^[:alnum:]_])(RUN_SHELL|RunShell|run_shell)([^[:alnum:]_]|$)' server client crates integration-tests packaging web
reject_matches 'generic WRITE_FILE command capability is present' '(^|[^[:alnum:]_])(WRITE_FILE|WriteFile|write_file)([^[:alnum:]_]|$)' server client crates integration-tests packaging web
reject_matches 'generic SYSTEMD_UNIT command capability is present' '(^|[^[:alnum:]_])(SYSTEMD_UNIT|SystemdUnit|systemd_unit)([^[:alnum:]_]|$)' server client crates integration-tests packaging web
reject_matches 'generic certificate issue/install protocol is present' 'CertificateIssueRequest|INSTALL_CERTIFICATE|InstallCertificate' server client crates web/openapi
reject_matches 'generic APPLY_CADDY_FRAGMENT command capability is present' '(^|[^[:alnum:]_])(APPLY_CADDY_FRAGMENT|ApplyCaddyFragment|apply_caddy_fragment)([^[:alnum:]_]|$)' server client crates integration-tests packaging web
reject_matches 'generic SET_ENV command capability is present' '(^|[^[:alnum:]_])(SET_ENV|SetEnv|set_env)([^[:alnum:]_]|$)' server client crates integration-tests packaging web

spreadsheet_file="$(find . -path './target' -prune -o -path './node_modules' -prune -o -type f \( -iname '*.xlsx' -o -iname '*.xls' -o -iname '*.ods' \) -print -quit)"
[[ -z ${spreadsheet_file} ]] || fail "spreadsheet file is present: ${spreadsheet_file}"

[[ ! -e packaging/client/rootfs/usr/lib/systemd/user/natsume-session-agent.service ]] \
  || fail 'Session Agent systemd user unit is present'
require_desktop='packaging/client/rootfs/etc/xdg/autostart/org.natsume.SessionAgent.desktop'
[[ -f ${require_desktop} ]] || fail 'Session Agent XDG Autostart entry is missing'
grep -Fxq 'Exec=/usr/bin/natsume-session-agent --autostart' "${require_desktop}" \
  || fail 'Session Agent XDG Autostart Exec is incorrect'
reject_matches 'direct low-level GUI stack dependency is present in production manifests' \
  '(^|[^[:alnum:]_])(winit|softbuffer|tiny-skia|cosmic-text)([^[:alnum:]_]|$)' \
  Cargo.toml client/*/Cargo.toml server/Cargo.toml crates/*/Cargo.toml
reject_matches 'systemd-user Session Agent launcher is referenced' \
  'systemctl[[:space:]]+--user|systemd-run[[:space:]]+--user|graphical-session\.target' \
  client packaging/client/rootfs/usr/lib packaging/client/rootfs/etc integration-tests

printf 'policy-scan: ok\n'
