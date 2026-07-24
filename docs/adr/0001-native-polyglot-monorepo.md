# ADR-0001: Native polyglot monorepo

## Decision

Use one Git repository, Cargo virtual workspace for Rust, pnpm workspace for Web, a thin root justfile, and nFPM for final Debian composition. Do not introduce Moon/Nx/Turbo/Bazel initially.

## Revisit trigger

Revisit only after multiple Web packages exist and measured CI data shows affected-build or remote-cache value. Cargo and pnpm remain authoritative even then.
