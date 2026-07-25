# Machine Identity and Vault Recovery

> 适用：identity unavailable/mismatch、vault decrypt/tamper、configured-disk copy  
> 关键不变量：`INV-IDENTITY-01`、`INV-IDENTITY-02`、`INV-SECRET-01`

此 runbook 的首要目标是保护身份和证据，不是尽快让 daemon 变绿。

## 1. 立即行动

1. 停止敏感 Command 和新的 Enrollment；
2. 不删除或改写：
   - `/var/lib/natsume/identity/machine-hardware-id`
   - Client vault
   - Command journal
   - certificate/key material
3. 记录 service status、boot ID、Machine short ID、stable ErrorCode；
4. 复制只读诊断 metadata/hash；不要复制 secret 明文；
5. 确认是否发生硬件更换、系统盘复制、重装、权限/磁盘损坏或 key 丢失；
6. 必要时隔离 Device 网络，但保留本地 evidence。

## 2. 分类

### A. 首次启动，无 identity-bound artifact

- 硬件候选唯一且质量合格：允许正常首次创建；
- 候选不可用/冲突：修复采集/权限/硬件，不能创建 fallback ID。

### B. 已有 artifact，当前 ID 匹配

继续检查 vault：

- owner/mode；
- root key material；
- ciphertext format/version；
- tamper/wrong-key；
- 磁盘/IO；
- 最近 upgrade/restore。

### C. 已有 artifact，当前 ID 不可用

- 检查 collector 权限、kernel/sysfs、Helper/D-Bus、硬件 source；
- 修复后重新收集；
- 在身份恢复前不打开 vault；
- 不得选择单个弱候选继续。

### D. 已有 artifact，当前 ID 不匹配

可能是：

- configured-disk copy；
- 主板/设备更换；
- 错误磁盘；
- identity file 篡改；
- collector semantics 变化。

停止。若确为新硬件，执行 [Device replacement](device-replacement.md)，不修改旧 ID。

### E. ID 匹配，vault 解密失败

- 检查 key material availability/permissions；
- 检查 vault version/migration；
- 验证 backup/restore 是否完整；
- 使用受控副本运行离线诊断；
- 不创建新 vault 覆盖旧值；
- 若 key 丢失且无法恢复，按安全事件和 replacement/reset 决策处理。

## 3. 恢复路径

### 修复权限/collector

1. 修复 package-defined permission/policy；
2. 重新运行纯 collector/identity probe；
3. 比较 derived ID；
4. 只有完全匹配才允许 daemon 继续；
5. 检查 vault和 Observed。

### 恢复 vault backup

1. 确认 backup 对应同一 Device/Machine ID/format；
2. 验证 hash、key custody 和 backup time；
3. 停止 daemon；
4. 保留当前损坏副本；
5. 在 staging path 验证 decrypt/tamper；
6. 原子替换；
7. 启动 daemon；
8. 检查 certificate、credential revision、LKG；
9. 发送 Observed；
10. 必要时显式重新 sync，不能自动。

### 无法恢复

选择其一并正式批准：

- Device replacement；
- 单生命周期 reset（只在全局场景）；
- 保持退役/隔离。

不得通过删除 identity/vault 触发无审计 Enrollment。

## 4. 成功判定

- 当前 Machine ID 唯一、质量合格且与持久化匹配；
- vault 可解密并通过 tamper/version checks；
- Device key/certificate匹配；
- Command journal 可恢复；
- Observed 明确；
- Server Device lifecycle 未被错误复制；
- 恢复动作有审计/evidence；
- 未泄露 secret/raw serial。

## 5. Evidence

```text
DEVICE_PK=
BOOT_ID=
ERROR_CODE=
PERSISTED_ID_HASH=
CURRENT_ID_HASH=
COLLECTOR_RESULT=
VAULT_FORMAT=
VAULT_HASH=
BACKUP_ID=
RECOVERY_ACTION=
CERT_CHECK=
JOURNAL_CHECK=
FINAL_OBSERVED=
OWNER=
REVIEWER=
```
