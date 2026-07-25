# ADR-0003: Direct nFPM packaging

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

项目需要 Server/Client Deb，并要求 package content、mode、systemd、D-Bus、XDG 和 Caddy artifact 可审计。复制到自建 staging tree 或在 package script 内重新构建会形成重复路径。

## Decision

直接使用 nFPM，将已经由 Cargo/pnpm 构建的固定 artifact 映射到 Deb。nFPM 配置不拥有产品构建；postinstall 不下载组件或生成 CA/private key。

## Alternatives

- 自定义 dpkg staging shell：路径和 mode 易漂移。
- Cargo deb/npm packaging：难以统一多 binary、Web、Caddy 和系统文件。
- 容器交付：不符合当前工作站/systemd 部署边界。

## Consequences

### Positive

- package manifest 可读；
- 构建与打包职责分离；
- install/upgrade smoke 容易自动化。

### Negative / trade-offs

- 需要自行维护 maintainer script 和 lifecycle tests；
- nFPM 能力边界可能需要少量预构建文件。

## Evidence and revisit trigger

若 nFPM 无法表达已冻结的 Debian lifecycle 或安全属性，可在有 package smoke 对比后重新评估。

## References

- [repository-layout.md](../repository-layout.md)
- [dependency-policy.md](../dependency-policy.md)
- [backup-restore-and-upgrade.md](../runbooks/backup-restore-and-upgrade.md)
