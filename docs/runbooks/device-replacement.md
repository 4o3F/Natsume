# Device Replacement

> 适用：硬件更换、主板身份变化、工作站永久退役  
> 关键不变量：`INV-IDENTITY-02`、`INV-CERT-01`、`INV-SECRET-02`

Replacement 不是修改 Machine Hardware ID，也不是复制旧 vault/key。

## 1. 前提

- 原 DevicePk、Machine short ID、Seat/binding 和 certificate inventory 已确认；
- 新硬件可独立收集 Machine ID；
- 原设备状态（在线、丢失、损坏）已记录；
- 有权限 retire/delete、unbind、enroll、bind、sync；
- 当前竞赛和 Seat universe 保持不变；
- 对丢失/疑似泄露设备准备证书撤销。

## 2. 保护原设备

1. 停止向原 Device 创建新 Command；
2. 记录最新 Observed、Drift、pending Command；
3. 取消尚未 receipt 的命令；
4. 对已 receipt 命令等待安全终态或标记 incident；
5. 从 Seat unbind；
6. retire 原 Device；
7. 必要时撤销 Device/Gateway certificate；
8. 保存 audit 和 certificate serial。

不得把原 DevicePk 重新指向新 Machine ID。

## 3. 新设备

1. clean install Client package；
2. 验证新 Machine ID；
3. 确认没有复制旧 identity file、vault、Device/Gateway private key；
4. 按 [Enrollment and mTLS](enrollment-and-mtls.md) 创建新 Device lifecycle；
5. 验证 authenticated QUIC 和 Observed；
6. 绑定原 Seat；
7. 显式执行 `SYNC_STATE`；
8. 验证 Gateway/Caddy；
9. 人工显式执行 `SYNC_SECRET`；
10. 验证 credential revision、session/home 和数据面。

## 4. 原设备清理

原设备可访问时：

1. 保留所需事件证据；
2. 终止受管 session；
3. 清理 Home；
4. 通过批准的 local reset/secure erase 流程处理 vault/key；
5. remove/purge package 或重装系统；
6. 确认旧 certificate 已失效；
7. 更新资产清单。

丢失时只做 Server 侧 retire/revoke，不能假设本地材料已销毁。

## 5. 成功判定

- 原 Device 非 active 且无 binding；
- 新 DevicePk/Machine ID 独立；
- 旧 certificate 不再认证；
- 新 Device完成 state/secret；
- Caddy/Observed/Drift 正常；
- 没有 key/vault/identity copy；
- audit 将原/新 lifecycle 关联为 replacement。

## 6. 回滚

若新设备未就绪：

- 不恢复已确认泄露/失效的旧 certificate；
- 可以在原设备仍可信且未 retire/revoke 前暂缓切换；
- 一旦完成 retire/revoke，只通过新的明确 Enrollment 恢复；
- 不得通过数据库编辑复活。

## 7. Evidence

```text
OLD_DEVICE_PK=
NEW_DEVICE_PK=
SEAT=
OLD_CERT_SERIALS=
NEW_CERT_SERIALS=
UNBIND_EVENT=
RETIRE_EVENT=
REVOKE_EVENT=
NEW_ENROLLMENT=
SYNC_STATE_COMMAND=
SYNC_SECRET_COMMAND=
FINAL_OBSERVED=
OWNER=
REVIEWER=
```
