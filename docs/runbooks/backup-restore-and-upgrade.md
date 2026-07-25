# Backup, Restore and Upgrade

> 适用：Server/Client 发布升级、数据库/vault 恢复、rollback  
> 关键不变量：`INV-SECRET-01`、`INV-IDENTITY-02`、`INV-COMMAND-01`

## 1. 备份范围

### Server 备份范围

- SQLite database/WAL 一致快照；
- Server vault key custody metadata（按批准机制，不能与 ciphertext 无控制地同存）；
- Server config；
- public certificate/chain 和 inventory；
- package/version；
- migration version；
- audit/evidence locator；
- Web/static config；
- 不包含 offline root private key。

### Client 备份范围

常规 fleet backup 不复制 Device/Gateway private key或 Client vault 到 Server。现场维修备份仅按批准的同机恢复策略，并绑定 Machine ID/Device lifecycle。

记录：

- identity file hash；
- vault format/hash；
- package version；
- certificate serial/fingerprint；
- journal/LKG metadata；
- 不导出秘密明文。

## 2. Server backup

1. 记录 commit/package/schema；
2. 暂停或使用 SQLite online backup API确保一致；
3. 包含 WAL所需状态；
4. 计算 hash；
5. 加密并限制访问；
6. 将 vault key custody与数据备份分离；
7. 执行定期 restore test；
8. 记录 retention和删除。

只复制 `.db` 而忽略 WAL/一致性不构成有效备份。

## 3. Server restore

1. 在隔离环境验证 backup hash/version；
2. 准备相同或兼容 package；
3. 恢复 database和必要 config；
4. 按批准方式提供 vault key；
5. 运行 migration/format validation；
6. 验证 decrypt/tamper；
7. 启动 Server；
8. 检查 Seat/Device/binding/Target/Command/Audit；
9. 检查 certificate inventory和 endpoint；
10. 让测试/少量 Device重连；
11. 验证 Observed/Drift和 duplicate Command幂等；
12. 正式切换；
13. 保留旧实例回滚窗口。

恢复不把历史 Command重发成新 ID，也不自动 secret sync。

## 4. Client 同机恢复

只适用于相同物理机器且 Machine ID 完全匹配：

1. 停止 Device Daemon；
2. 记录 current identity/vault hash；
3. 验证 backup对应同一 Machine ID、DevicePk和format；
4. 保留当前副本；
5. 在 staging验证 key/decrypt；
6. 原子恢复；
7. 启动；
8. 验证 Device/Gateway key/certificate、journal、LKG、credential revision；
9. 连接 Server并发送 Observed；
10. 显式处理 Drift。

不同硬件不能用该流程，必须 Device replacement。

## 5. Upgrade

### 前置

- release note和 migration review；
- package checksum；
- backup/restore test；
- target OS lifecycle evidence；
- capacity/disk；
- certificate expiry；
- pending Operation/Command；
- rollback plan；
- maintenance window。

### Server 升级

1. 完成一致备份；
2. 停止/Drain；
3. 安装 package；
4. 运行 migration；
5. 验证 vault；
6. 启动；
7. health/API/Web；
8. Device reconnect；
9. Command/Observed；
10. audit；
11. 扩大流量。

### Client 升级

1. 记录 identity/vault/cert/journal/LKG；
2. 安装/upgrade；
3. 确认 endpoint保留；
4. 重启/boot；
5. identity-before-vault；
6. mTLS reconnect；
7. Observed；
8. Caddy；
9. Agent/XDG；
10. Home/session；
11. 无 user unit/Identity Guard/runtime download。

## 6. Rollback

Rollback 不等于自动数据库 downgrade。

- 若新 package 在 migration 前失败：恢复旧 package；
- 若 migration 已提交：按 release-specific forward-fix或已验证 backup restore；
- 保持 evidence和新旧 version；
- 不把旧 binary指向不兼容新 schema；
- Client vault format变化必须有兼容 reader或原子 backup restore；
- 证书/identity不得因 rollback重新生成；
- 失败期间保持数据面 LKG或BLOCKED。

## 7. 成功判定

- restore可用而非仅 backup存在；
- vault可解密且无 secret leakage；
- schema/package匹配；
- Device身份不变；
- pending Command幂等恢复；
- Caddy/Session/Home正常；
- audit和版本记录完整；
- rollback路径已验证。

## 8. Evidence

```text
RELEASE=
OLD_VERSION=
NEW_VERSION=
SCHEMA_BEFORE=
SCHEMA_AFTER=
BACKUP_ID=
BACKUP_SHA256=
RESTORE_TEST=
VAULT_CHECK=
ROLLBACK_TEST=
DEVICE_SAMPLE=
OBSERVED_RESULT=
OWNER=
REVIEWER=
```
