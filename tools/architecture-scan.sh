#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'architecture scan failed: %s\n' "$1" >&2
  exit 1
}

[[ -f docs/architecture.md ]] || fail 'docs/architecture.md is missing'
while IFS= read -r doc; do
  case ${doc} in
  docs/README.md | docs/architecture.md | docs/operations-deployment.zh-CN.md | \
    docs/prd-gnome-dual-session.zh-CN.md | \
    docs/gnome-session-image-requirements.zh-CN.md | docs/gnome-session-image-configuration.zh-CN.md) ;;
  *) fail "unexpected maintained document: ${doc}; development records belong in context/" ;;
  esac
done < <(rg --files --hidden docs -g '*.md')

[[ ! -e crates/error-code ]] || fail 'the global error-code crate still exists'
if rg -n 'natsume[-_]error[-_]code|crates/error-code' \
  Cargo.toml Cargo.lock server client crates; then
  fail 'the removed global error-code crate is still referenced'
fi

proto_root='crates/device-protocol/proto'
if rg -n '^\s*reserved\b' "${proto_root}"; then
  fail 'active-development protobuf still reserves deleted fields or numbers'
fi
if rg -n '^\s*(enum|message)\s+ErrorCode\b' "${proto_root}"; then
  fail 'Device Control error_code must remain an open string token'
fi
if rg -n '\b(Command|CommandStatus|ControlEnvelope|CredentialBundle|DeviceToken|ObservedStateSnapshot)\b' \
  "${proto_root}"; then
  fail 'legacy Device Control protocol symbols remain'
fi
