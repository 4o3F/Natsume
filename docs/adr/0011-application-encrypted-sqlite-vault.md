# ADR-0011: Application-encrypted SQLite vault

> Status: `ACCEPTED`  
> Scope: Natsume V2  
> Note: Client 侧条款（随机 root key + HKDF 绑定的 Client vault）已由 [ADR-0026](0026-client-secrets-as-permission-files.md) 替代为权限文件；Server vault 条款维持有效。

## Context

Server/Client 需要在 SQLite 或本地状态中保存密码和 key material。依赖全盘加密、系统 credential 传递或明文文件不能提供应用级边界和可测试格式。

## Decision

使用应用层 AEAD vault。Server vault 在应用内加密；Client 使用随机 32-byte root key，并用 Machine ID 作为 HKDF 绑定输入之一。秘密不通过 systemd credentials、env 或 argv。

## Alternatives

- 明文 SQLite + FS permissions：root/backup 暴露面更大。
- systemd credentials：不匹配持久业务 vault 和当前部署约束。
- Machine ID 直接作 key：低熵且不是秘密。
- 外部通用 secret manager：增加现场依赖和离线复杂度。

## Consequences

### Positive

- 格式和 tamper 可测试；
- 数据库备份单独不直接暴露明文；
- identity-bound Client recovery 明确。

### Negative / trade-offs

- 需要 key custody、rotation 和 migration；
- 应用内仍会短暂出现明文。

## Evidence and revisit trigger

任何替代 secret store 必须证明离线、backup、rotation、package 和恢复能力不降低 `INV-SECRET-01/IDENTITY-02`。

## References

- [security-recovery.md](../security-recovery.md)
