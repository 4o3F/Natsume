#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repository_root}"

fail() {
    printf 'policy-scan: %s\n' "$*" >&2
    exit 1
}

reject_matches() {
    local description="$1"
    local pattern="$2"
    shift 2

    local matches
    local status
    if matches=$(grep -RInE \
        --exclude-dir=node_modules \
        --exclude-dir=target \
        --exclude-dir=dist \
        --exclude-dir=playwright-report \
        --exclude-dir=test-results \
        --exclude=policy-scan.sh \
        -- "${pattern}" "$@"); then
        printf '%s\n' "${matches}" >&2
        fail "${description}"
    else
        status=$?
        if [[ ${status} -ne 1 ]]; then
            fail "scanner error while checking ${description}"
        fi
    fi
}

private_key_pattern='BEGIN ([A-Z ]+ )?PRIVATE KEY'
printf '%s\n' '-----BEGIN PRIVATE KEY-----' | grep -Eq "${private_key_pattern}" \
    || fail 'private-key canary was not detected'

while IFS= read -r usage; do
    reference="${usage##*@}"
    if [[ ! ${reference} =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        fail "GitHub Action is not pinned to an exact release version: ${usage}"
    fi
done < <(
    grep -RhoE 'uses:[[:space:]]+[^[:space:]#]+@[^[:space:]#]+' .github/workflows \
        | sed -E 's/^uses:[[:space:]]+//'
)

reject_matches \
    'placeholder CI command remains' \
    'Implementation requirement|run:[[:space:]]*echo[[:space:]].*(placeholder|Pin actions|Run frozen)' \
    .github/workflows

reject_matches \
    'private-key material is present in first-party paths' \
    "${private_key_pattern}" \
    .github server client crates integration-tests packaging web

reject_matches \
    'credential-shaped token is present in first-party paths' \
    'AKIA[0-9A-Z]{16}|gh[pousr]_[0-9A-Za-z]{20,}' \
    .github server client crates integration-tests packaging web

reject_matches \
    'systemd credential directive is present' \
    'LoadCredential=|SetCredential=|ImportCredential=' \
    packaging/client/rootfs packaging/server/rootfs

reject_matches \
    'Identity Guard product surface is present' \
    'natsume-identity-guard|identity_guard|IdentityGuard' \
    server client crates integration-tests packaging

reject_matches \
    'maintainer script performs a network or package-manager download' \
    '(^|[^[:alnum:]_])(curl|wget|aria2c)([^[:alnum:]_]|$)|https?://|cargo[[:space:]]+install|pnpm[[:space:]]+(add|install)|npm[[:space:]]+install|go[[:space:]]+install' \
    packaging/client/scripts packaging/server/scripts packaging/client/debconf

reject_matches \
    'first-party Rust code depends on anyhow or thiserror' \
    '(^|[^[:alnum:]_])(anyhow|thiserror)([^[:alnum:]_]|$)' \
    server client crates integration-tests

reject_matches \
    'spreadsheet adapter surface is present' \
    '(^|[^[:alnum:]_])(calamine|umya-spreadsheet|exceljs|sheetjs|xlsx-js-style)([^[:alnum:]_]|$)' \
    server client crates integration-tests web package.json Cargo.toml

reject_matches \
    'generic certificate issue/install protocol is present' \
    'CertificateIssueRequest|INSTALL_CERTIFICATE|InstallCertificate' \
    server client crates web/openapi

spreadsheet_file="$(find . \
    -path './target' -prune -o \
    -path './node_modules' -prune -o \
    -type f \( -iname '*.xlsx' -o -iname '*.xls' -o -iname '*.ods' \) \
    -print -quit)"
if [[ -n ${spreadsheet_file} ]]; then
    fail "spreadsheet file is present: ${spreadsheet_file}"
fi

printf 'policy-scan: ok\n'
