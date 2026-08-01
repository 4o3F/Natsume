# ADR-0026: Client secrets as permission-guarded files

> Status: `ACCEPTED`  
> Scope: Natsume V2 Client 侧秘密与凭据存储  
> Supersedes: —（替代 [ADR-0011](0011-application-encrypted-sqlite-vault.md) 的 Client vault 条款；Server vault 条款维持不变）  
> Superseded by: —

## Context

[ADR-0011](0011-application-encrypted-sqlite-vault.md) 为 Client 定义了应用层 AEAD vault（随机 32-byte root key + Machine ID HKDF 绑定 + format version + 迁移/轮换）。按 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md) 重估：root key 与密文位于同一块盘，能取得密文者（root 或物理接触）必然能取得 key——而这两类攻击者都在威胁模型之外（T2）。在场攻击者只有非 root 选手，root-owned 文件权限已完全阻断。Client 加密不击败任何在模型内的攻击者，却带来 key 保管、格式迁移与恢复机制。这与全行业以权限文件存放 TLS 私钥（`/etc/ssl/private`）的实践一致。

[ADR-0023](0023-wss-control-channel-with-device-token.md) 与 [ADR-0024](0024-domjudge-autologin-via-xheaders.md) 落地后，Client 侧秘密收敛为四类小文件，SQLite vault 载体也失去必要性。

## Decision

1. Client 侧秘密与凭据以 **root-owned 权限文件** 存放，全部使用原子写（temp + fsync + rename）：

   | 内容 | 路径示例 | 权限 |
   |---|---|---|
   | Device Token | `/var/lib/natsume/enrollment/device-token` | `0600 root:root` |
   | Gateway private key + leaf/chain | `/var/lib/natsume/gateway/` | `0640 root:natsume-gateway` |
   | Seat 凭据文件（`SYNC_SECRET` 真相源） | `/var/lib/natsume/credentials/` | `0600 root:root` |
   | 渲染出的含凭据 Caddy 配置 | `/var/lib/natsume/caddy/` | `0640 root:natsume-gateway` |

   LKG 配置等非秘密状态同目录树存放，权限按内容分级。
2. **不做应用层加密、不做 HKDF 绑定、不维护 vault format 迁移框架**。文件带最小版本头以便将来演进，但不预建迁移机制。
3. **identity-before-credentials 顺序保留**（[ADR-0006](0006-daemon-integrated-machine-identity-startup.md)、`INV-IDENTITY-02`）：硬件身份校验通过前不读取、不使用任何 enrollment 产物；身份 mismatch 或凭据文件损坏均 fail closed，不自动重建、不自动 re-enroll。
4. **Server vault 维持 ADR-0011 原样**（应用层 AEAD）：它防的数据库备份/落盘泄露是模型内的真实威胁。
5. 内存卫生规则维持：secrecy/zeroize 类型、不 Debug/serde 到通用结构、错误链不含值。

## Alternatives

- **维持 Client AEAD vault**：机制齐全但无对应攻击者；key custody 本身成为新的失败点（key 损坏 = 全部凭据不可用）。
- **systemd credentials**：ADR-0011 已拒绝，约束不变。
- **TPM 绑定**：异构硬件（F1）无法保证 TPM 存在与质量；超出威胁模型需求。

## Consequences

### Positive

- 删除 key 保管、格式迁移、wrong-key/tamper 恢复整条机制链；
- 凭据损坏的恢复路径统一为"窗口重开 re-enrollment / 重新 `SYNC_SECRET`"，与既有流程重合；
- 文件权限模型可被 package smoke 与负向测试直接验证。

### Negative / trade-offs

- 磁盘明文：物理拿盘即得凭据——已在威胁模型外（T2），且原方案的同盘 key 并未实质改变该结论；
- 若未来威胁模型纳入物理攻击，需要引入外部信任根（TPM/外置 key），届时以新 ADR 替代本决策。

## Evidence and revisit trigger

- 接受前需要：package smoke 验证权限/属主/mode；非 root 用户读取失败的负向测试；原子写崩溃注入（半写不可见）。
- 重开条件：威胁模型纳入物理/root 攻击者，或部署出现可依赖的硬件信任根基线。

## References

- [ADR-0006](0006-daemon-integrated-machine-identity-startup.md)
- [ADR-0011](0011-application-encrypted-sqlite-vault.md)
- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [security-recovery.md](../security-recovery.md)
