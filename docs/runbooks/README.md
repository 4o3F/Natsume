# Natsume Runbooks

Runbook 只描述操作、验证、回滚和证据。架构和安全规则分别由 [Architecture](../architecture.md)、[Contracts](../contracts.md)、[Security & recovery](../security-recovery.md) 拥有。遇到冲突时，必须停止操作并修正文档，不在现场自行创造例外。

## 当前状态

当前仓库仍是 Phase 0 工程基线。具体 runbook 在对应 Phase 实现后编写，目前不保留未建系统的目标操作流程。涉及尚未实现的 binary、API 或状态机的步骤是目标操作契约，不是当前可执行性或 Gate 证据；每个发布候选必须用实际命令、包路径和目标环境 evidence 复核后才能投入现场。

## 使用规则

1. 确认适用环境、版本和当前 Device/Command/epoch；
2. 保存日志、状态、证书 fingerprint 和 correlation ID；
3. 对 destructive step 建立备份和双人确认；
4. 不把密码、private key、原始 hardware serial 或完整 vault 放入 ticket/evidence；
5. 逐项执行，不跳过 precondition；
6. 用 Observed、Drift、certificate inspection、Caddy health 和 audit 验证结果；
7. 失败后按 rollback/stop condition 处理，不通过删除 vault/identity/journal“重试”；
8. 运行后记录 commit、environment、owner、reviewer、date 和 limitations。

## 通用事件记录

```text
INCIDENT_OR_CHANGE_ID:
DATE:
ENVIRONMENT:
COMMIT_OR_PACKAGE_VERSION:
SERVER_ID:
DEVICE_PK:
MACHINE_SHORT_ID:
SEAT:
OPERATION_ID:
COMMAND_ID:
SESSION_EPOCH:
HOME_EPOCH:
ERROR_CODE:
PRECONDITION_RESULT:
ACTIONS:
EVIDENCE:
FINAL_OBSERVED:
FINAL_DRIFT:
AUDIT_EVENT:
OWNER:
REVIEWER:
LIMITATIONS:
```

只填写适用字段，所有敏感值脱敏。
