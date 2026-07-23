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
    grep -Fqx 'name = "protoc-bin-vendored"' Cargo.lock

install:
    pnpm install --frozen-lockfile

lockfile:
    pnpm install --lockfile-only

fmt:
    cargo fmt --all --check
    pnpm --filter @natsume/web format:check

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo deny check
    pnpm --filter @natsume/web lint
    pnpm --filter @natsume/web typecheck

unit:
    cargo test --workspace --all-features
    pnpm --filter @natsume/web test

api:
    cargo run -p natsume-server --bin export-openapi -- web/openapi/natsume.openapi.json
    pnpm --filter @natsume/web api:generate

diagrams:
    pnpm diagrams

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
    pnpm --filter @natsume/web format:check
    pnpm --filter @natsume/web lint
    pnpm --filter @natsume/web typecheck
    pnpm --filter @natsume/web test
    pnpm --filter @natsume/web build

ci-contracts: install
    cargo run -p natsume-server --locked --bin export-openapi -- web/openapi/natsume.openapi.json
    pnpm --filter @natsume/web api:generate
    git diff --exit-code -- web/openapi/natsume.openapi.json web/src/api/generated/schema.d.ts
    cargo test -p natsume-integration-tests --locked
    pnpm diagrams

ci-policy:
    bash integration-tests/policy-scan.sh

ci-packages:
    bash packaging/ci-package-smoke.sh

verify: toolchain install fmt lint unit api diagrams

package-server:
    nfpm package --packager deb --config packaging/server/nfpm.yaml --target dist/packages/

package-client:
    grep -Exq '[0-9a-f]{64}  caddy' packaging/client/caddy.sha256
    nfpm package --packager deb --config packaging/client/nfpm.yaml --target dist/packages/

package: package-server package-client
