# ADR-0029: Right-sizing control-plane machinery

> Status: `ACCEPTED`  
> Scope: Natsume V2 控制面辅助机制（事件分发、批量聚合、版本计数、RBAC、Caddy 控制、治理证据）  
> Supersedes: —  
> Superseded by: —

## Context

以下机制各自成立于通用多消费者/多团队假设，按 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)（单进程 Server、1–3 操作员、3 人团队、单场地）重估后均无对应场景。它们共享同一个裁剪依据，故合并为一个 ADR；每条独立可执行。

## Decision

1. **ChangeEvent/outbox 与 SSE 删除**。Transactional outbox 解决"本地事务 + 外部系统"双写；此处生产者与全部消费者同在 natsume-server 进程内，消费端是至多 3 个操作员浏览器。替代：Web Panel 轮询 + 进程内通知；AuditEvent 保留且与领域事务同事务写入（审计原子性不变）。
2. **Operation 聚合层延后**。V2 只保留 Command（单 Device durable intent）与其 status/重试计数；批量操作 = 批量创建 Command，进度视图由查询聚合。Attempt 不作为独立建模概念，投递观察记录为 Command 元数据。出现真实的跨设备聚合业务需求（取消语义、分批推进）时再以新 ADR 引入。
3. **`ConfigurationGeneration` 不再独立计数**，由 `(ContestConfigurationRevision, 站点 policy 版本)` 确定性派生；陈旧判定语义不变（Device 侧仍按代际拒绝陈旧 `SYNC_STATE`）。独立计数器收敛为：`ContestConfigurationRevision`、`AssignmentRevision`、`CredentialRevision`、`SessionEpoch`、`HomeEpoch`。
4. **RBAC 收敛为两个固定角色**：`admin`（全部操作）与 `viewer`（只读）。不建角色/权限编辑模型；角色变化仍审计。
5. **Caddy 控制走"渲染文件 + 校验 + 重载"**：Daemon 渲染完整配置文件 → `caddy validate` → 原子替换 → systemd path unit 触发 reload → 本地健康检查，失败回滚 LKG 文件。**不使用 Caddy Admin API**。
6. **治理证据降级**：Gate 证据从 8 字段格式降级为"指向 CI run / commit / artifact 的链接 + 一行结论"；nightly 全包生命周期 CI 改为每周与发版前。"文件存在 ≠ 通过、截图 ≠ 可复现日志、VM ≠ 物理身份证据"等原则保留。

## Alternatives

- **保留 outbox/SSE**：为单进程内通信付出投递语义、失败补偿与表维护；轮询在 3 个消费者规模下开销可忽略。
- **保留 Operation/Attempt 建模**：普通 CRUD 与批量 Command 查询已覆盖当前全部 UI 需求；提前建模违反 YAGNI。
- **保留独立 generation 计数**：与内容 revision 的区分在"Target 为纯派生"的模型下不携带额外信息，反而多一处冻结/校验点。
- **保留 Admin API**：动态重配能力无消费者；文件 + reload 与 LKG 回滚的故障模型更简单且已有 path unit。

## Consequences

### Positive

- 删除 outbox 表、投递语义、SSE 通道与对应测试；
- 计数器从 6 个减至 5 个（其一为纯派生值）；
- Caddy 控制面故障模型收敛为文件系统原子性 + reload 结果两点；
- 治理开销与 3 人团队匹配。

### Negative / trade-offs

- Web 状态刷新有轮询延迟（秒级，操作员场景可接受）；
- 未来出现多消费者事件需求（外部系统集成）时需重新引入 outbox——届时是真实场景，机制有对象；
- 批量操作的取消/分批语义受限于 Command 粒度。

## Evidence and revisit trigger

- 接受前需要：审计与领域事务同事务的原子性测试；Caddy validate/reload 失败回滚 LKG 的故障注入；派生 generation 的陈旧拒绝测试。
- 重开条件：出现进程外事件消费者、真实批量聚合业务、或角色需求超出两级。

## References

- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [architecture.md](../architecture.md)
- [domain-model.md](../domain-model.md)
- [state-and-execution.md](../state-and-execution.md)
