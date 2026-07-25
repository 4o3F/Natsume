# Session Lock Recovery

> 适用：lock/unlock/terminate 失败、陈旧 Agent、epoch conflict  
> 关键不变量：`INV-SESSION-01`

Session lock 是桌面会话状态，不是网络隔离。任何 lock/unlock 恢复都不得调用或改变 Caddy。

## 1. 记录当前状态

```text
DEVICE_PK
SEAT
BOOT_ID
LOGIND_SESSION_ID
SESSION_EPOCH
HOME_EPOCH
AGENT_PID
AGENT_LEASE
REQUESTED_ACTION
COMMAND_ID
ERROR_CODE
CADDY_ADMIN_CALL_COUNT
CADDY_CONFIG_HASH
CONFIGURATION_GENERATION
CADDY_STATUS
```

在执行恢复前记录 Caddy 四项基线。

## 2. 校验当前会话

1. Device active 且 current Observed 可用；
2. logind session 属于 fixed contest user；
3. session graphical；
4. Seat/UID/boot ID 与 SessionEpoch 一致；
5. Agent 属于该 session 且 lease 有效；
6. Home 状态允许当前 action；
7. Command 绑定当前 epoch。

任何不一致都拒绝旧 action，并重新发现当前 session。不要重写 epoch 让陈旧请求通过。

## 3. Lock 失败

- 检查 desktop lock capability；
- 检查 Daemon → Agent/local adapter结果；
- 检查 display/session是否消失；
- 检查 stable ErrorCode；
- 若 lock request 已到 desktop但结果未知，重新观察真实 session 状态；
- 不要重复执行不同 Command 直到确认原 Command 终态；
- 不能通过 Caddy BLOCKED 代替 lock。

## 4. Unlock 失败

- 确认当前 session确实 locked；
- 确认 unlock capability在目标 desktop被批准；
- 校验 operator authorization和 current epoch；
- 拒绝陈旧 Agent/Command；
- 若目标 desktop不支持程序化 unlock，按冻结 capability转人工/terminate路径；
- 不得保存或模拟用户密码；
- 不得通过启动新 Agent绕过 desktop policy。

## 5. Terminate/replacement

当 session 无法安全恢复：

1. 创建绑定当前 epoch 的 terminate action；
2. 等待真实 logind session结束；
3. 使旧 Agent lease 过期；
4. 执行 Home cleanup/recovery；
5. 创建新的 SessionEpoch；
6. 启动新受管 session；
7. 验证 XDG Agent；
8. 不复用旧 action；
9. 记录 audit/Observed。

## 6. Caddy 不变验证

恢复前后比较：

- Admin call count；
- config hash；
- configuration generation；
- BLOCKED/READY status；
- 状态页 payload。

任何变化都判定为 Session/Caddy coupling 缺陷，停止 Gate 签收。状态页不得出现 `session_locked`。

## 7. 成功判定

- 当前 session/epoch 明确；
- lock/unlock/terminate结果与 Observed一致；
- 旧 Agent/action被拒绝；
- Home状态安全；
- Caddy四项不变；
- audit、Command terminal和 stable ErrorCode完整。

## 8. Evidence

```text
COMMAND_ID=
OLD_SESSION_EPOCH=
NEW_SESSION_EPOCH=
ACTION_RESULT=
LOGIND_BEFORE_AFTER=
AGENT_LEASE_BEFORE_AFTER=
CADDY_CALLS_BEFORE_AFTER=
CADDY_HASH_BEFORE_AFTER=
CADDY_GENERATION_BEFORE_AFTER=
CADDY_STATUS_BEFORE_AFTER=
OBSERVED_SEQUENCE=
AUDIT_EVENT=
OWNER=
REVIEWER=
```
