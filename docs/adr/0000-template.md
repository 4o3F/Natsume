# ADR-0000：架构决策记录模板

> 状态：`TEMPLATE`，本文件不是已接受的架构决策  
> 文件名：`NNNN-kebab-case-title.md`，实际决策从 `0001` 开始。

## 使用规则

1. 状态仅使用 `PROPOSED`、`ACCEPTED`、`REJECTED`、`SUPERSEDED`。
2. 所有事实必须引用权威设计、目标环境或探针证据。
3. 平台输入未确认时引用 `docs/supported-platform.md` 中的 `ENV-UNFROZEN` 或 `ENV-PROPOSED` 条目；禁止使用无前缀的 `FROZEN` / `UNFROZEN` 表示平台冻结状态。
4. 涉及证书时必须分别描述 Device Identity 与 Gateway `SYNC_STATE` 边界。
5. 禁止粘贴 private key、密码、原始硬件序列号或完整敏感 payload。
6. G0 相关 ADR 必须映射 `REQ-P0-*`、Probe 和 `G0-*`。

---

# ADR-NNNN：标题

## 元数据

| 字段 | 值 |
|---|---|
| ADR ID | `ADR-NNNN` |
| 状态 | `PROPOSED` |
| 日期 | `YYYY-MM-DD` |
| Decision owner | `ROLE_*` |
| Reviewers | `ROLE_*` |
| 关联需求 | `REQ-P0-*` |
| 关联 Probe/Gate | Probe `A`–`F` / `G0-*` |
| 替代的 ADR | 无；若有则填写 `ADR-NNNN` |

## 1. 上下文（Context）

说明需要解决的问题、触发条件和约束。

- 当前事实与证据：
- 相关平台条目：
- 不作决策的后果：
- 安全、时间和发布约束：

## 2. 决策（Decision）

使用可测试的语言描述选择和明确拒绝项。

> 我们决定：
>
> 我们明确不采用：

### 2.1 证书边界（如适用）

| 边界 | 决策 |
|---|---|
| Device Identity certificate | Enrollment 参与范围；Gate 叙述别名 `READY-DEVICE-ID`（不是 wire/API/DB 字段） |
| Gateway certificate | authenticated QUIC + `SYNC_STATE` 范围；Gate 叙述别名 `READY-GATEWAY-CERT`（不是 wire/API/DB 字段） |
| 禁止项 | Gateway material in Enrollment、TOFU、通用证书签发接口等 |

## 3. 备选方案（Alternatives）

| 方案 | 描述 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| A | | | | |
| B | | | | |
| C（可选） | | | | |

## 4. 失败与恢复（Failure / Recovery）

- 失败表现：
- 检测方式和稳定错误码：
- Fail-closed 行为：
- 恢复或回滚步骤：
- 对已安装包、配置、证书、Device 和数据的影响：

## 5. 安全影响（Security Impact）

- 信任边界变化：
- 密钥和证书生命周期：
- Secret/PII/硬件证据处理：
- 是否引入 TOFU、systemd credentials、runtime download、Identity Guard：
- 威胁模型与日志/审计影响：

## 6. 测试影响（Test Impact）

| 层级 | 所需验证 |
|---|---|
| PR | |
| Nightly/VM | |
| Physical/Desktop Lab | |
| Probe/G0 | |

列出测试文件、命令、正反用例和预期证据路径。

## 7. 迁移影响（Migration Impact）

- 配置和 debconf/preseed：
- systemd/D-Bus/package topology：
- 数据库 migration：
- OpenAPI/Protobuf/D-Bus contract：
- 升级、回滚和恢复：
- 需要同步更新的设计、runbook 和 Gate 文档：

## 8. 后果与残余限制

### 正向后果

-

### 负向后果与成本

-

### 已知限制 / Non-claims

-

## 9. 追踪与证据

| 类型 | 定位符 |
|---|---|
| Requirements | `REQ-P0-*` |
| Supported platform | `docs/supported-platform.md` 中的章节与字段名 |
| Lab assets | `ENV-*` / `HW-*` |
| Probe report | `docs/probes/*.md` |
| Gate | `G0-*` |
| CI/Test evidence | 尚无时写 `NONE`，不得预填 PASS |

## 10. 审批

| 角色 | 姓名 | 日期 | 结论 |
|---|---|---|---|
| Decision owner | | | |
| Architecture reviewer | | | |
| Security/QA reviewer | | | |

## 11. 修订历史

| 日期 | 修改 | 作者角色 |
|---|---|---|
| YYYY-MM-DD | 初稿 | `ROLE_*` |

## Phase 0 ADR 索引

| ID | 主题 | 状态 |
|---|---|---|
| `ADR-0001` | Native monorepo 与 direct nFPM | 未创建 |
| [`ADR-0002`](./0002-error-code-registry.md) | ErrorCode registry | `PROPOSED` |
| `ADR-0003` | IP-SAN 安装 endpoint | 未创建 |
| `ADR-0004` | Device/Gateway certificate ladder | 未创建 |
| `ADR-0005` | Caddy version/module/source pin | 未创建 |
| `ADR-0006` | OverlayFS 与 staged-copy backend | 未创建 |
