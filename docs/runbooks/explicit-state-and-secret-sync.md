# Explicit State and Secret Sync

> 适用：Binding/配置/credential 已提交后，将变化显式应用到 Device  
> 关键不变量：`INV-STATE-01`、`INV-SECRET-02`、`INV-COMMAND-01`、`INV-DATAPLANE-01`

`SYNC_STATE` 与 `SYNC_SECRET` 是两个独立动作。不得把 password 放入 state sync，也不得因 Drift 自动触发 secret sync。

## 1. 前提

- operator 有相应权限；
- Device active；
- latest Observed freshness 已检查；
- Target 和 Drift 可见；
- binding、assignment revision、configuration generation 已确认；
- secret sync 时 credential revision 已确认；
- pending/conflicting Command 已处理；
- Machine identity/vault 无 fail-closed 状态。

## 2. `SYNC_STATE`

1. 在 UI 查看 Target 与最新 Observed；
2. 检查 Device/Seat/account、assignment revision、configuration generation；
3. 查看将应用的非秘密变化；
4. 明确创建 `SYNC_STATE`；
5. 记录 Operation/Command ID；
6. 等待 Device durable receipt；
7. 观察 Gateway request/证书阶段（如需要）；
8. 观察 Caddy staging/activation；
9. 等待 terminal status；
10. 获取新 Observed；
11. 检查 Drift 收敛；
12. 检查 Caddy READY 或明确 BLOCKED 原因。

失败时按 Command terminal ErrorCode 分流，不直接重发新 Command。相同 Command ID retry 由系统处理。

## 3. `SYNC_SECRET`

1. 确认 `SYNC_STATE` 已使当前 binding/configuration 就绪；
2. 核对 Seat/account、assignment revision、credential revision；
3. 确认 UI 不展示 password；
4. 明确创建 `SYNC_SECRET`；
5. 记录 Command ID；
6. 等待 durable receipt；
7. Device 写入 Client vault；
8. 等待 redacted terminal status；
9. 获取新 Observed，确认 installed credential revision；
10. 检查 AuditEvent 和日志无 secret；
11. 必要时通过受管浏览器验证登录，而不导出密码。

## 4. 常见结果

| Error/状态 | 解释 | 行动 |
|---|---|---|
| stale assignment | binding 已变化 | 刷新 Target，创建新 Command |
| stale generation | 配置已变化 | 刷新后重新 `SYNC_STATE` |
| stale credential revision | 密码版本已变化 | 刷新后重新 `SYNC_SECRET` |
| Gateway request conflict | request/SPKI 或 active command 不一致 | 转 Gateway runbook |
| vault failure | 本地秘密存储不可用 | 转 identity/vault recovery |
| Caddy validation/load failure | 数据面未激活 | 转 Caddy runbook |
| Device offline | 未投递/连接中断 | 保持 Command，等待重连/策略重试 |
| duplicate Command | 返回既有状态 | 不创建额外业务动作 |

## 5. 停止条件

- Observed 陈旧且无法确认 Device；
- Device lifecycle 非 active；
- identity/vault error；
- assignment/generation/revision 与 UI 不一致；
- 错误 Device/Seat；
- 任何界面、日志或 API 暴露 password；
- Caddy 候选状态不满足验证；
- 需要通过手工数据库/本地文件修改才能继续。

## 6. 成功判定

State：

- Command `SUCCEEDED`；
- Observed generation/revision 匹配；
- Gateway certificate 状态正确；
- Caddy READY 或预期 BLOCKED；
- Drift 收敛；
- LKG 更新。

Secret：

- Command `SUCCEEDED`；
- Observed credential revision 匹配；
- password 未可观察；
- audit redacted；
- 无自动 secret sync。

## 7. Evidence

```text
DEVICE_PK=
SEAT=
TARGET_GENERATION=
ASSIGNMENT_REVISION=
CREDENTIAL_REVISION=
SYNC_STATE_COMMAND=
SYNC_STATE_RESULT=
GATEWAY_SERIAL=
CADDY_HASH=
SYNC_SECRET_COMMAND=
SYNC_SECRET_RESULT=
FINAL_OBSERVED=
FINAL_DRIFT=
AUDIT_EVENTS=
OWNER=
REVIEWER=
```
