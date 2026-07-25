# Readiness Rehearsal

> 适用：G7/生产发布前、重大版本或现场变更前  
> 目标：由非作者按文档完成从空环境到恢复的完整演练

## 1. 前提

- 所有目标平台 `ENV-FROZEN`；
- Gate G0–G6 已签署；
- release candidate artifact/checksum；
- 生产等价但不含真实秘密的环境；
- 至少一台 Server、代表性 Client、双桌面和 DOMjudge；
- 6 台 Machine ID evidence已完成；
- PKI test/ceremony material；
- 备份存储；
- 操作员、管理员、安全和观察员角色；
- 故障注入批准；
- 停止条件和现场联系人。

## 2. 演练记录

```text
REHEARSAL_ID=
RELEASE=
COMMIT_SHA=
DATE=
ENVIRONMENTS=
PARTICIPANTS=
OBSERVER=
START_TIME=
END_TIME=
```

每步记录实际耗时、结果、evidence、文档偏差和人工判断。

## 3. 正常路径

1. provision Server control certificate；
2. 安装 Server；
3. 创建 operator/RBAC；
4. 安装 Client；
5. 配置 endpoint；
6. 导入 CSV；
7. Enroll Device；
8. mTLS/Observed；
9. bind Seat；
10. `SYNC_STATE`；
11. Gateway/Caddy READY；
12. `SYNC_SECRET`；
13. 受管浏览器/DOMjudge；
14. Home prepare；
15. session start；
16. Agent hidden/lazy UI；
17. lock/unlock/terminate；
18. audit、Target/Observed/Drift检查。

## 4. 故障场景

至少执行：

- 错误 IP/CA；
- anonymous QUIC；
- Server离线/重启；
- Device断线/重启；
- duplicate Command；
- stale generation/revision；
- Gateway SPKI conflict；
- bad certificate/SAN/expiry；
- Caddy validate/load失败；
- DOMjudge unavailable；
- vault wrong-key/tamper模拟；
- configured-disk copy或 identity mismatch；
- Agent crash/display lost；
- stale SessionEpoch；
- Home prepare中断/disk full；
- backup/restore；
- upgrade/rollback；
- Device replacement。

故障注入不得使用生产 key/password。

## 5. 恢复验证

每个场景验证：

- 是否 fail closed；
- 是否保留安全 LKG；
- 是否需要人工介入；
- 是否有 stable ErrorCode；
- Command是否幂等；
- Observed/Drift是否收敛；
- audit是否完整；
- 文档是否能由非作者执行；
- 是否出现未记录特例；
- 恢复耗时是否满足目标。

## 6. 发布阻塞

以下任一项阻塞发布：

- 安全不变量失败；
- secret/private key泄漏；
- 需要TOFU、手工数据库编辑或删除vault/identity；
- Session操作改变Caddy；
- Home状态不确定仍启动session；
- restore未验证；
- 目标桌面/OS未签收；
- 关键runbook只有作者能完成；
- 未解释的数据丢失或身份变化；
- Gate/evidence不一致。

## 7. 结果

```text
RESULT=PASS|FAIL
NORMAL_PATH=
FAULT_SCENARIOS=
RESTORE=
UPGRADE_ROLLBACK=
RUNBOOK_DEFECTS=
PRODUCT_DEFECTS=
RESIDUAL_RISKS=
FOLLOW_UP_OWNERS=
RELEASE_RECOMMENDATION=
SIGNATURES=
```
