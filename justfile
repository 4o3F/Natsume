set shell := ["bash", "-euo", "pipefail", "-c"]

install:
    pnpm install --frozen-lockfile

lockfile:
    pnpm install --lockfile-only

fmt: shell-format
    cargo fmt --all --check
    pnpm --filter @natsume/web format:check

shell-format:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(<tools/shfmt.version)"
    test "$(shfmt --version)" = "v${version}"
    git ls-files -z -- '*.sh' | xargs -0r shfmt -d

web-audit:
    pnpm audit --audit-level high --registry https://registry.npmjs.org

openapi-lint:
    pnpm --filter @natsume/web openapi:lint

secret-scan:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(<tools/gitleaks.version)"
    test "$(gitleaks version)" = "${version}"
    gitleaks git --redact --no-banner --no-color --config .gitleaks.toml .
    gitleaks dir --redact --no-banner --no-color --config .gitleaks.toml .

lint: web-audit
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo deny check
    pnpm --filter @natsume/web lint
    pnpm --filter @natsume/web typecheck

unit:
    cargo test --workspace --all-features
    pnpm --filter @natsume/web test

api:
    cargo run -p natsume-server --bin export-openapi -- web/openapi/natsume.openapi.json
    pnpm --filter @natsume/web openapi:lint
    pnpm --filter @natsume/web api:generate

diesel-schema:
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(diesel --version | sed -n 's/^ Version: //p')"
    test "$version" = "2.3.12"
    # print-schema silently emits unformatted output when rustfmt is missing.
    rustfmt --version >/dev/null
    temp_dir="$(mktemp -d /tmp/natsume-diesel-schema.XXXXXX)"
    trap 'rm -rf -- "$temp_dir"' EXIT
    database="$temp_dir/schema.sqlite3"
    generated="$temp_dir/schema.rs"
    diesel --database-url "$database" --config-file /dev/null migration run --migration-dir crates/natsume-server/migrations
    diesel --database-url "$database" --config-file crates/natsume-server/diesel.toml print-schema > "$generated"
    diff -u -- "$generated" crates/natsume-server/diesel/schema.rs

build:
    cargo build --workspace --release --locked
    pnpm --filter @natsume/web build

e2e:
    pnpm --filter @natsume/web e2e

ci-rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo test --workspace --all-features --locked --lib --bins --tests
    cargo test --workspace --all-features --locked --doc
    cargo deny check

ci-web: install
    pnpm audit --audit-level high --registry https://registry.npmjs.org
    pnpm --filter @natsume/web format:check
    pnpm --filter @natsume/web lint
    pnpm --filter @natsume/web typecheck
    pnpm --filter @natsume/web test
    pnpm --filter @natsume/web build

schema-contract:
    #!/usr/bin/env bash
    set -euo pipefail
    output="$(mktemp /tmp/natsume-schema-contract.XXXXXX)"
    trap 'rm -f -- "$output"' EXIT
    cargo test -p natsume-server --locked --test schema_contract -- --color never --format pretty | tee "$output"
    if ! grep -Eq '^test result: ok\. [1-9][0-9]* passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;' "$output"; then
        printf '%s\n' 'schema_contract must execute at least one test, with none failed, ignored, or filtered out.' >&2
        exit 1
    fi

ci-contracts: install diesel-schema schema-contract
    cargo run -p natsume-server --locked --bin export-openapi -- web/openapi/natsume.openapi.json
    pnpm --filter @natsume/web openapi:lint
    pnpm --filter @natsume/web api:generate
    git diff --exit-code -- web/openapi/natsume.openapi.json web/src/api/generated/schema.d.ts

ci-policy: shell-format secret-scan

ci-packages:
    bash packaging/ci-package-smoke.sh

verify: install fmt lint unit api diesel-schema secret-scan

package-server:
    mkdir -p dist/packages
    envsubst '${ARCH} ${VERSION} ${RUST_RELEASE_DIR}' < packaging/server/nfpm.yaml | nfpm package --packager deb --config /dev/stdin --target dist/packages/

package-client:
    grep -Exq '[0-9a-f]{64}  caddy' packaging/client/caddy.sha256
    python3 packaging/check-image-inputs.py
    mkdir -p dist/packages
    python3 packaging/client/prepare-avatar.py dist/packages/author-avatar.png
    AUTHOR_AVATAR_FILE="$PWD/dist/packages/author-avatar.png" envsubst '${ARCH} ${VERSION} ${RUST_RELEASE_DIR} ${CADDY_BIN} ${AUTHOR_AVATAR_FILE}' < packaging/client/nfpm.yaml | nfpm package --packager deb --config /dev/stdin --target dist/packages/
    python3 packaging/check-image-inputs.py --deb "dist/packages/natsume-client_${VERSION}_${ARCH}.deb"

package: package-server package-client
