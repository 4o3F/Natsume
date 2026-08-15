set shell := ["bash", "-euo", "pipefail", "-c"]

toolchain:
    test "$(node --version)" = "v24.1.0"
    test "$(pnpm --version)" = "11.1.0"
    rustc --version | grep -Eq '^rustc 1\.97\.1 '
    grep -Exq '2\.11\.4' packaging/client/caddy.version
    grep -Exq '[0-9a-f]{64}  caddy_2\.11\.4_linux_amd64\.tar\.gz' packaging/client/caddy.archive.sha256
    grep -Exq '[0-9a-f]{64}  caddy' packaging/client/caddy.sha256
    grep -Exq '2\.47\.0' packaging/nfpm.version
    grep -Exq '[0-9a-f]{64}  nfpm_2\.47\.0_Linux_x86_64\.tar\.gz' packaging/nfpm.sha256
    grep -Exq '3\.13\.1' tools/shfmt.version
    grep -Exq '[0-9a-f]{64}  shfmt_v3\.13\.1_linux_amd64' tools/shfmt.sha256
    grep -Exq '8\.30\.1' tools/gitleaks.version
    grep -Exq '[0-9a-f]{64}  gitleaks_8\.30\.1_linux_x64\.tar\.gz' tools/gitleaks.sha256
    grep -Fqx 'name = "protoc-bin-vendored"' Cargo.lock

docs-validate:
    node --test docs/verification/markdown.test.mjs
    git ls-files -z -- '*.md' | xargs -0r node docs/verification/validate-links.mjs
    node docs/verification/validate-markdown.mjs docs README.md

install:
    pnpm install --frozen-lockfile

lockfile:
    pnpm install --lockfile-only

fmt: shell-format
    cargo fmt --all --check
    pnpm --filter @natsume/web format:check

shell-format:
    #!/usr/bin/env bash
    version="$(<tools/shfmt.version)"
    test "$(shfmt --version)" = "v${version}"
    git ls-files -z -- '*.sh' | xargs -0r shfmt -d

web-audit:
    pnpm audit --audit-level high --registry https://registry.npmjs.org

openapi-lint:
    pnpm --filter @natsume/web openapi:lint

secret-scan:
    #!/usr/bin/env bash
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

diagrams:
    pnpm diagrams

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
    diesel --database-url "$database" --config-file /dev/null migration run --migration-dir server/migrations
    diesel --database-url "$database" --config-file server/diesel.toml print-schema > "$generated"
    diff -u -- "$generated" server/src/db/schema.rs

build:
    cargo build --workspace --release --locked
    pnpm --filter @natsume/web build

integration:
    cargo test -p natsume-integration-tests

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
    ! grep -rq '@apply' web/dist/assets
    grep -rq 'min-h-screen' web/dist/assets

ci-contracts: install docs-validate diesel-schema
    cargo run -p natsume-server --locked --bin export-openapi -- web/openapi/natsume.openapi.json
    pnpm --filter @natsume/web openapi:lint
    pnpm --filter @natsume/web api:generate
    git diff --exit-code -- web/openapi/natsume.openapi.json web/src/api/generated/schema.d.ts
    cargo test -p natsume-integration-tests --locked
    pnpm diagrams

ci-policy: shell-format secret-scan
    bash integration-tests/policy-scan.sh

ci-packages:
    bash packaging/ci-package-smoke.sh

verify: toolchain install fmt lint unit api diesel-schema diagrams docs-validate secret-scan

package-server:
    nfpm package --packager deb --config packaging/server/nfpm.yaml --target dist/packages/

package-client:
    grep -Exq '[0-9a-f]{64}  caddy' packaging/client/caddy.sha256
    nfpm package --packager deb --config packaging/client/nfpm.yaml --target dist/packages/

package: package-server package-client
