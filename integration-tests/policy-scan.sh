#!/usr/bin/env bash
set -euo pipefail
repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repository_root}"

fail() {
  printf 'policy-scan: %s\n' "$*" >&2
  exit 1
}
reject_matches() {
  local description="$1" pattern="$2"
  shift 2
  local matches status
  if matches=$(grep -RInE --exclude-dir=node_modules --exclude-dir=target --exclude-dir=dist \
    --exclude-dir=playwright-report --exclude-dir=test-results --exclude=policy-scan.sh \
    -- "${pattern}" "$@"); then
    printf '%s\n' "${matches}" >&2
    fail "${description}"
  else
    status=$?
    [[ ${status} -eq 1 ]] || fail "scanner error while checking ${description}"
  fi
}

# Same as reject_matches, but drops lines matching an allow pattern first. Used where the
# forbidden token is a short word that also appears in a legitimate unrelated construct.
reject_matches_except() {
  local description="$1" pattern="$2" allow="$3"
  shift 3
  local raw status matches
  raw=$(grep -RInE --exclude-dir=node_modules --exclude-dir=target --exclude-dir=dist \
    --exclude-dir=playwright-report --exclude-dir=test-results --exclude=policy-scan.sh \
    -- "${pattern}" "$@") || {
    status=$?
    [[ ${status} -eq 1 ]] || fail "scanner error while checking ${description}"
    return 0
  }
  matches=$(printf '%s\n' "${raw}" | grep -vE -- "${allow}") || {
    status=$?
    [[ ${status} -eq 1 ]] || fail "scanner error while filtering ${description}"
    return 0
  }
  [[ -z ${matches} ]] || {
    printf '%s\n' "${matches}" >&2
    fail "${description}"
  }
}

private_key_pattern='BEGIN ([A-Z ]+ )?PRIVATE KEY'
printf '%s\n' '-----BEGIN PRIVATE KEY-----' | grep -Eq "${private_key_pattern}" || fail 'private-key canary was not detected'
sqlx_pattern='(^|[^[:alnum:]_])sqlx([^[:alnum:]_]|$)'
printf '%s\n' 'sqlx::query' | grep -Eq "${sqlx_pattern}" || fail 'SQLx canary was not detected'
print_macro_pattern='(^|[^[:alnum:]_])(print|println|eprint|eprintln|dbg)!'
printf '%s\n' 'println!("canary")' | grep -Eq "${print_macro_pattern}" || fail 'Rust print-macro canary was not detected'
test_support_pattern='(^|[^[:alnum:]_])test_support([^[:alnum:]_]|$)'
printf '%s\n' 'mod test_support;' | grep -Eq "${test_support_pattern}" || fail 'test-support module canary was not detected'
discarded_rollback_pattern='let[[:space:]]+_[[:space:]]*=[[:space:]]*rollback_transaction'
printf '%s\n' 'let _ = rollback_transaction(connection);' | grep -Eq "${discarded_rollback_pattern}" || fail 'discarded rollback canary was not detected'
diesel_anonymous_trait_pattern='(^|[^[:alnum:]_])(Connection|RunQueryDsl|ExpressionMethods|QueryDsl|OptionalExtension|SimpleConnection|MigrationHarness)[[:space:]]+as[[:space:]]+_'
printf '%s\n' 'use diesel::{Connection as _, RunQueryDsl as _};' | grep -Eq "${diesel_anonymous_trait_pattern}" || fail 'anonymous Diesel trait canary was not detected'
legacy_http_auth_helper_pattern='(^|[^[:alnum:]_])authenticated_operator([^[:alnum:]_]|$)'
printf '%s\n' 'authenticated_operator(headers)' | grep -Eq "${legacy_http_auth_helper_pattern}" || fail 'legacy HTTP authentication helper canary was not detected'
http_session_authentication_pattern='operator::authenticate_session'
printf '%s\n' 'operator::authenticate_session(database, credential)' | grep -Eq "${http_session_authentication_pattern}" || fail 'HTTP session authentication canary was not detected'
http_problem_module_pattern='(^|[^[:alnum:]_])ApiProblem([^[:alnum:]_]|$)|mod[[:space:]]+problem[[:space:]]*;|problem::ApiProblem'
for canary in 'ApiProblem' 'mod problem;' 'problem::ApiProblem'; do
  printf '%s\n' "${canary}" | grep -Eq "${http_problem_module_pattern}" || fail 'legacy HTTP problem-module canary was not detected'
done
discarded_logging_initialization_pattern='let[[:space:]]+_[[:alnum:]_]*[[:space:]]*=[[:space:]]*tracing::subscriber::set_global_default'
printf '%s\n' 'let _initialization = tracing::subscriber::set_global_default(subscriber);' | grep -Eq "${discarded_logging_initialization_pattern}" || fail 'discarded logging initialization canary was not detected'
legacy_logging_initialization_pattern='tracing::subscriber::set_global_default'
printf '%s\n' 'tracing::subscriber::set_global_default(subscriber)' | grep -Eq "${legacy_logging_initialization_pattern}" || fail 'legacy logging initialization canary was not detected'
discarded_vault_cleanup_pattern='let[[:space:]]+_[[:alnum:]_]*[[:space:]]*=[[:space:]]*fs::remove_file\(&self\.path\)'
printf '%s\n' 'let _cleanup_result = fs::remove_file(&self.path);' | grep -Eq "${discarded_vault_cleanup_pattern}" || fail 'discarded vault cleanup canary was not detected'
silent_password_verifier_pattern='let[[:space:]]+Ok\(verifier\)[[:space:]]*=[[:space:]]*frozen_argon2\(\)[[:space:]]*else'
printf '%s\n' 'let Ok(verifier) = frozen_argon2() else {' | grep -Eq "${silent_password_verifier_pattern}" || fail 'silent password verifier canary was not detected'
web_title_branch_pattern='\.title[[:space:]]*[!=]==?|\.title\.(includes|startsWith|endsWith|match|search)\('
for canary in 'if (error.title === "boom") {' 'error.title.includes("boom")'; do
  printf '%s\n' "${canary}" | grep -Eq "${web_title_branch_pattern}" || fail 'Web title-branch canary was not detected'
done
low_level_gui_dependency_pattern='(^|[^[:alnum:]_])(winit|softbuffer|tiny-skia|cosmic-text)([^[:alnum:]_]|$)'
slint_winit_feature_allow_pattern='^Cargo\.toml:[0-9]+:slint = \{ version = "1\.15", default-features = false, features = \["compat-1-2", "std", "backend-winit-x11", "renderer-skia"\] \}$'
mtls_server_verifier_pattern='WebPkiClientVerifier|ClientCertVerifier|with_client_cert_verifier'
mtls_presenting_client_pattern='with_client_auth_cert'
ed25519_seed_pattern='SigningKey::(from_bytes|try_from|from_keypair_bytes|from)[[:space:]]*\('
ed25519_seed_allow_pattern='^integration-tests/tests/ordinary_wss_ed25519_feasibility/protocol\.rs:[0-9]+:[[:space:]]*let (source|key|wrong_key) = SigningKey::from_bytes\(&(CONTROL_KEY_SEED|RFC8032_SEED|\[0x22; 32\])\);$'
ed25519_ordinary_verify_pattern='(\.|::)[[:space:]]*verify[[:space:]]*\('
ed25519_dependency_pattern='ed25519-dalek'
ed25519_dependency_allow_pattern='^(Cargo\.toml:[0-9]+:ed25519-dalek = \{ version = "2\.2", default-features = false, features = \["alloc", "pkcs8", "zeroize"\] \}|integration-tests/Cargo\.toml:[0-9]+:ed25519-dalek\.workspace = true)$'
ed25519_source_pattern='ed25519_dalek'
ed25519_source_allow_pattern='^integration-tests/tests/ordinary_wss_ed25519_feasibility/protocol\.rs:[0-9]+:'
printf '%s\n' 'Cargo.toml:1:winit = "0.30"' | grep -Eq "${low_level_gui_dependency_pattern}" || fail 'low-level GUI dependency canary was not detected'
printf '%s\n' 'Cargo.toml:1:slint = { version = "1.15", default-features = false, features = ["compat-1-2", "std", "backend-winit-x11", "renderer-skia"] }' | grep -Eq "${slint_winit_feature_allow_pattern}" || fail 'Slint winit feature allow canary was not detected'
if printf '%s\n' 'Cargo.toml:1:winit = "0.30"' | grep -Eq "${slint_winit_feature_allow_pattern}"; then
  fail 'Slint winit feature allow pattern accepted a bare winit dependency'
fi
for canary in 'WebPkiClientVerifier' 'impl ClientCertVerifier' '.with_client_cert_verifier(verifier)'; do
  printf '%s\n' "${canary}" | grep -Eq "${mtls_server_verifier_pattern}" || fail 'mTLS server-verifier canary was not detected'
done
printf '%s\n' '.with_client_auth_cert(certs, key)' | grep -Eq "${mtls_presenting_client_pattern}" ||
  fail 'static mTLS client-identity canary was not detected'
for allowed in '.with_no_client_auth()' 'impl ResolvesClientCert for NoCertificateResolver' '.with_client_cert_resolver(Arc::new(NoCertificateResolver))'; do
  if printf '%s\n' "${allowed}" | grep -Eq "${mtls_server_verifier_pattern}|${mtls_presenting_client_pattern}"; then
    fail 'mTLS pattern rejected an explicit no-certificate configuration'
  fi
done
for canary in 'SigningKey::from_bytes(&seed)' 'SigningKey::from(&seed)' 'SigningKey::try_from(seed)' 'SigningKey::from_keypair_bytes(&bytes)'; do
  printf '%s\n' "${canary}" | grep -Eq "${ed25519_seed_pattern}" || fail 'Ed25519 seed construction canary was not detected'
done
if printf '%s\n' 'server/src/auth.rs:1:SigningKey::from_bytes(&seed)' | grep -Eq "${ed25519_seed_allow_pattern}"; then
  fail 'Ed25519 seed allow pattern accepted a production path'
fi
printf '%s\n' 'server/Cargo.toml:1:ed25519-dalek.workspace = true' | grep -Eq "${ed25519_dependency_pattern}" || fail 'Ed25519 dependency canary was not detected'
for allowed in \
  'Cargo.toml:42:ed25519-dalek = { version = "2.2", default-features = false, features = ["alloc", "pkcs8", "zeroize"] }' \
  'integration-tests/Cargo.toml:29:ed25519-dalek.workspace = true'; do
  printf '%s\n' "${allowed}" | grep -Eq "${ed25519_dependency_allow_pattern}" ||
    fail 'Ed25519 dependency allow pattern rejected the exact isolated declaration'
done
for rejected in \
  'server/Cargo.toml:1:ed25519-dalek.workspace = true' \
  'Cargo.toml:42:ed25519-dalek = { version = "2.2", default-features = true, features = ["alloc", "pkcs8", "zeroize"] }' \
  'Cargo.toml:42:ed25519-dalek = { version = "2.3", default-features = false, features = ["alloc", "pkcs8", "zeroize"] }'; do
  if printf '%s\n' "${rejected}" | grep -Eq "${ed25519_dependency_allow_pattern}"; then
    fail 'Ed25519 dependency allow pattern accepted a non-approved declaration'
  fi
done
printf '%s\n' 'server/src/auth.rs:1:use ed25519_dalek::SigningKey;' | grep -Eq "${ed25519_source_pattern}" || fail 'Ed25519 source canary was not detected'
if printf '%s\n' 'server/src/auth.rs:1:use ed25519_dalek::SigningKey;' | grep -Eq "${ed25519_source_allow_pattern}"; then
  fail 'Ed25519 source allow pattern accepted production code'
fi
for canary in 'verifying_key.verify(message, signature)' 'verifying_key.verify (message, signature)' 'Verifier::verify(&key, message, signature)' '<VerifyingKey as Verifier<Signature>>::verify(&key, message, signature)'; do
  printf '%s\n' "${canary}" | grep -Eq "${ed25519_ordinary_verify_pattern}" || fail 'ordinary Ed25519 verify canary was not detected'
done
if printf '%s\n' 'verifying_key.verify_strict(message, signature)' | grep -Eq "${ed25519_ordinary_verify_pattern}"; then
  fail 'ordinary Ed25519 verify pattern rejected verify_strict'
fi

while IFS= read -r usage; do
  reference="${usage##*@}"
  [[ ${reference} =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "GitHub Action is not pinned to an exact release version: ${usage}"
done < <(grep -RhoE 'uses:[[:space:]]+[^[:space:]#]+@[^[:space:]#]+' .github/workflows | sed -E 's/^uses:[[:space:]]+//')

reject_matches 'placeholder CI command remains' 'Implementation requirement|run:[[:space:]]*echo[[:space:]].*(placeholder|Pin actions|Run frozen)' .github/workflows
reject_matches 'private-key material is present in first-party paths' "${private_key_pattern}" .github server client crates integration-tests packaging web docs
reject_matches_except 'Ed25519 dependency escaped the isolated feasibility probe' \
  "${ed25519_dependency_pattern}" "${ed25519_dependency_allow_pattern}" \
  Cargo.toml server client crates integration-tests packaging web
reject_matches_except 'Ed25519 source usage escaped the isolated feasibility probe' \
  "${ed25519_source_pattern}" "${ed25519_source_allow_pattern}" \
  server client crates integration-tests packaging web
reject_matches_except 'Ed25519 signing-key seed construction is outside the isolated public test vector' \
  "${ed25519_seed_pattern}" "${ed25519_seed_allow_pattern}" server client crates integration-tests packaging web
reject_matches 'ordinary Ed25519 verification bypasses strict verification' \
  "${ed25519_ordinary_verify_pattern}" server client crates integration-tests
strict_verify_file='integration-tests/tests/ordinary_wss_ed25519_feasibility/protocol.rs'
grep -Fxq '    key.verify_strict(&proof_transcript(challenge, proof), &signature)' "${strict_verify_file}" ||
  fail 'live isolated proof verifier is not pinned to verify_strict'
reject_matches 'credential-shaped token is present' 'AKIA[0-9A-Z]{16}|gh[pousr]_[0-9A-Za-z]{20,}' .github server client crates integration-tests packaging web docs
reject_matches 'systemd credential directive is present' 'LoadCredential=|SetCredential=|ImportCredential=' packaging/client/rootfs packaging/server/rootfs
reject_matches 'Identity Guard product surface is present' 'natsume-identity-guard|identity_guard|IdentityGuard' server client crates integration-tests packaging
reject_matches 'maintainer script downloads at install time' '(^|[^[:alnum:]_])(curl|wget|aria2c)([^[:alnum:]_]|$)|https?://|cargo[[:space:]]+install|pnpm[[:space:]]+(add|install)|npm[[:space:]]+install|go[[:space:]]+install' packaging/client/scripts packaging/server/scripts packaging/client/debconf
reject_matches 'first-party Rust code depends on anyhow or thiserror' '(^|[^[:alnum:]_])(anyhow|thiserror)([^[:alnum:]_]|$)' server client crates integration-tests
reject_matches 'SQLx dependency or first-party Rust source usage is present' "${sqlx_pattern}" \
  Cargo.toml server/Cargo.toml server/src client/*/Cargo.toml client/*/src \
  crates/*/Cargo.toml crates/*/src integration-tests/Cargo.toml integration-tests/tests
reject_matches 'first-party Rust source uses direct print macros instead of tracing or an explicit writer' \
  "${print_macro_pattern}" server/src client/*/src client/*/examples crates/*/src integration-tests/*.rs integration-tests/tests
reject_matches 'test support is split from its owning tests module' \
  "${test_support_pattern}" server/src client/*/src crates/*/src integration-tests/tests
reject_matches 'database rollback result is discarded' \
  "${discarded_rollback_pattern}" server/src/db
reject_matches 'Diesel trait is imported anonymously instead of by name' \
  "${diesel_anonymous_trait_pattern}" server/src integration-tests/tests
reject_matches 'legacy handler-level HTTP authentication helper is present' \
  "${legacy_http_auth_helper_pattern}" server/src/http
reject_matches_except 'HTTP session authentication is outside the authentication middleware' \
  "${http_session_authentication_pattern}" \
  '^server/src/http/middleware/authentication\.rs:[0-9]+:' server/src/http
reject_matches 'legacy HTTP problem module or ApiProblem type is present' \
  "${http_problem_module_pattern}" server/src/http
reject_matches 'client logging initialization result is discarded' \
  "${discarded_logging_initialization_pattern}" client/*/src
reject_matches 'logging initialization bypasses tracing-subscriber try_init' \
  "${legacy_logging_initialization_pattern}" server/src/logging.rs client/*/src
reject_matches 'vault temporary-key cleanup result is discarded' \
  "${discarded_vault_cleanup_pattern}" server/src/vault.rs
reject_matches 'password verifier construction failure is collapsed into authentication failure' \
  "${silent_password_verifier_pattern}" server/src/application/operator.rs
reject_matches 'first-party Rust code parses Display text for behavior' '\.to_string\(\)[[:space:]]*\.(contains|starts_with|ends_with)|format!\([^;]*\)[[:space:]]*\.(contains|starts_with|ends_with)' server client crates integration-tests
reject_matches 'Web code branches on error title text instead of the stable code' "${web_title_branch_pattern}" web/src
reject_matches 'quinn dependency or source usage is present' '(^|[^[:alnum:]_])quinn([^[:alnum:]_]|$)' Cargo.toml server client crates integration-tests packaging web
reject_matches 'QUIC transport surface is present' '([Qq][Uu][Ii][Cc][[:space:]-]+(transport|control|gateway|session|client|listener|endpoint|over)|[Qq][Uu][Ii][Cc][[:space:]]*=|Device[[:space:]]+[Qq][Uu][Ii][Cc]|mTLS[[:space:]-]+[Qq][Uu][Ii][Cc])' server client crates integration-tests packaging web
reject_matches 'custom length-prefix framing is present' 'encode_frame|decode_frame|length[-_[:space:]]*(prefix|delimited)' server client crates integration-tests packaging web
reject_matches 'mTLS server-side client-certificate verifier is present' \
  "${mtls_server_verifier_pattern}" server client crates integration-tests packaging web
reject_matches 'static mTLS client identity is present' \
  "${mtls_presenting_client_pattern}" server client crates integration-tests packaging web
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

[[ ! -e packaging/client/rootfs/usr/lib/systemd/user/natsume-session-agent.service ]] ||
  fail 'Session Agent systemd user unit is present'
require_desktop='packaging/client/rootfs/etc/xdg/autostart/org.natsume.SessionAgent.desktop'
[[ -f ${require_desktop} ]] || fail 'Session Agent XDG Autostart entry is missing'
grep -Fxq 'Exec=/usr/bin/natsume-session-agent --autostart' "${require_desktop}" ||
  fail 'Session Agent XDG Autostart Exec is incorrect'
reject_matches_except 'direct low-level GUI stack dependency is present in production manifests' \
  "${low_level_gui_dependency_pattern}" "${slint_winit_feature_allow_pattern}" \
  Cargo.toml client/*/Cargo.toml server/Cargo.toml crates/*/Cargo.toml
reject_matches 'systemd-user Session Agent launcher is referenced' \
  'systemctl[[:space:]]+--user|systemd-run[[:space:]]+--user|graphical-session\.target' \
  client packaging/client/rootfs/usr/lib packaging/client/rootfs/etc integration-tests

cargo run --quiet --locked -p natsume-integration-tests --bin production-module-scan

printf 'policy-scan: ok\n'
