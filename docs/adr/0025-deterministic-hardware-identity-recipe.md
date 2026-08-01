# ADR-0025: Deterministic multi-source hardware identity recipe

> Status: `ACCEPTED`  
> Scope: Natsume V2 Machine Hardware ID  
> Supersedes: —（收窄 [ADR-0002](0002-library-first-machine-identity.md) 的来源策略；不动 library-first 结构与 [ADR-0010](0010-immutable-machine-id-and-device-lifecycle.md) 生命周期语义）  
> Superseded by: —

## Context

v1 以 MAC 地址为机器标识，发生过冲突（[ADR-0022](0022-deployment-facts-and-trust-assumptions.md) F1），多源综合是必要的。但原设计将其扩展为开放式框架：候选质量评分、动态来源准入（smbios/raw-cpuid/procfs/udev 增量评审）、以 6 台物理机 × 2 OEM 系列作为 G0 阻塞门禁。硬件识别真正要防的是**操作员误克隆已 provision 磁盘**这类运维事故（T2 排除了恶意本地攻击者），且 provisioning 窗口内操作员在场、fail closed 立即可见——护栏规格应与此匹配。

## Decision

1. **固定来源集合**：DMI system UUID、DMI 主板 serial、首块系统盘 serial。**MAC 地址排除**（v1 冲突证据）。来源集合变更必须修订本 ADR。
2. **确定性配方，无评分**：各来源规范化（大小写/空白/分隔符）后经 placeholder 黑名单过滤（`To be filled by O.E.M.`、`Default string`、全零、全 `F` 等已知无效值）；**至少 2 个来源有效**，否则 fail closed；有效来源按固定顺序拼接（缺失来源以显式空位标记参与拼接），在 Fleet namespace UUID 下派生 UUIDv5。
3. **启动匹配为严格相等**：重算 UUID 与持久化值不一致即 mismatch → fail closed，走替换/恢复 runbook。**不选择候选、不猜测"最可能是同一台"**（维持 [ADR-0010](0010-immutable-machine-id-and-device-lifecycle.md)、`INV-IDENTITY-02`）。已知代价：更换硬盘等部件会改变身份、需要窗口重开 re-enrollment——这是有意选择，可预测性优于聪明。
4. **验证降级**：取消"6 台物理机作为 G0 阻塞门禁"。替代证据：v1 事故机器与代表性硬件的匿名化 fixture 单元测试 + 首次 provisioning 时对全量异构机队的实地验证（操作员在场，fail closed 当场可见，且 [ADR-0023](0023-wss-control-channel-with-device-token.md) 的 re-enrollment 替换语义使克隆盘冲突自动显形）。
5. `machine-identity` pure crate、privileged collector 分离、匿名化 fixture 规则维持 [ADR-0002](0002-library-first-machine-identity.md) 不变。

## Alternatives

- **质量评分 + 动态来源框架**：为舰队产品设计的规格；护栏场景不需要，且评分引入"部分匹配继续运行"的灰色地带，与 fail closed 冲突。
- **单一来源（仅 DMI system UUID）**：异构硬件上 placeholder 概率不可忽略，单点失效即无法注册。
- **`/etc/machine-id`**：随镜像克隆复制，恰好在目标事故场景失效。
- **保留 MAC**：已有冲突证据。

## Consequences

### Positive

- 配方可用纸笔推演，fixture 测试穷举分支即可覆盖；
- 删除评分模型、来源准入评审与 6 台门禁，G0 缩短；
- 克隆盘事故检测由 Enrollment 替换语义免费提供。

### Negative / trade-offs

- 部件更换（含硬盘）触发 re-enrollment，维修流程多一步窗口操作；
- placeholder 黑名单需按实地硬件补充（修订走 fixture 与本 ADR 引用，不改配方结构）。

## Evidence and revisit trigger

- 接受前需要：黑名单/缺位/2-of-3/全缺 fixture 的决策表测试；克隆盘（相同派生 UUID）在 Enrollment 处的替换与审计验证。
- 重开条件：目标平台出现经过验证、跨磁盘克隆稳定的单一标准硬件身份 API（继承 ADR-0002 触发条件）；或实地 placeholder 率使 2-of-3 不可满足。

## References

- [ADR-0002](0002-library-first-machine-identity.md)
- [ADR-0010](0010-immutable-machine-id-and-device-lifecycle.md)
- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [security-recovery.md](../security-recovery.md)
