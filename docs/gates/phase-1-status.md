# Phase 1 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-15
> G1：`CLOSED`（7/7 PASS，owner 于 2026-08-15 签署关闭；已知限制与硬化待办随案挂账）

Phase 1（Control Domain）交付面已全部落地，本文件手写追踪 G1 证据收敛。条目通过需可定位 evidence（CI run / commit / artifact 链接 + 一行结论 + 日期），不得以文档存在、scaffold 或截图替代可复现结果；partial pass 记为未通过。

交付面锚点（实现完成、证据待登记）：Stage 5B operator surface（current-fact migration、operator auth、audit、operator API/OpenAPI）；Web navigation 与 auth shell（轮询，`d73674f`）；单监听器同源托管面板（`d337469`）；`reset-operator-password`（`bda92da`）；session TTL 契约对齐（`b8efe48`）。

## G1 条目（7 项）

| # | 条目 | 状态 |
|---|---|---|
| 1 | migration：空库首装幂等；升级路径 | `PASS`（升级半按已知限制挂账，见证据） |
| 2 | 领域不变量与事务原子性（bootstrap/session/reset/device lifecycle 单事务与回滚负向测试） | `PASS` |
| 3 | secret redaction（password/PHC/login name 不入日志与审计；错误 Display 泛化；canary 断言） | `PASS` |
| 4 | 两角色授权（admin/viewer 已挂载 route 全矩阵，viewer→admin action 稳定 `403`） | `PASS` |
| 5 | audit 原子性与词汇注册表对齐（与领域 mutation 同事务写入；写入器词汇与 §3.6.4 注册表逐字一致） | `PASS` |
| 6 | API generated clean diff（OpenAPI/TS/diesel golden，`ci-contracts` lane） | `PASS` |
| 7 | 模块依赖扫描（application/db 单向不变量、policy scan `module-dependency-scan`） | `PASS` |

## 已登记证据

全部条目锚定 [ci run 31887277032](https://github.com/4o3F/Natsume/actions/runs/31887277032)（head `bb58c53`，含全部 Phase 1 交付面，2026-08-15，全 lane 绿）：

- 条目 1：空库首装 migration 在全部 db 测试经 `connect_and_migrate` 真实执行；已迁移库的重开重迁由 repeat-bootstrap 与 reset 用例结构性覆盖（`create_if_missing=false` 打开后再跑 migration）；18 业务表 schema 契约 golden（`db.rs` exact business table contract）与 `ci-contracts` diesel clean diff 全绿。**已知限制**：跨版本升级 migration 无已发布前版 schema，用例随首个发布版建立（与 G0 条目 12 同源限制）。
- 条目 2：bootstrap 重复执行零写入、session termination repeat-safe、reset 审计失败回滚（重复 audit ID 注入）与双 operator 作用域隔离、device revoke/disable 三表单事务（`apply_lifecycle_mutations`）用例全绿（ci-rust lane，114 项 server 测试）。**已知限制**：terminate/expire 的多 operator 作用域负向覆盖列于「已登记待办」。
- 条目 3：结构化日志 login name/password canary（`http/tests.rs`）、store error Display/Debug canary（`db.rs`）、vault pointer canary（`contest.rs`）、reset 未知登录 Display/Debug 双 canary 全绿；policy scan credential 模式扫描同 run 通过。
- 条目 4：viewer→admin action 稳定 `403 AUTHORIZATION_DENIED`（revoke/disable）、admin/viewer 双角色读矩阵（seats/accounts/devices/bindings redacted current facts）、session 路由双角色用例全绿。
- 条目 5：全部生产审计写入器（create_first_admin/establish/terminate/expire/revoke/disable/close_provisioning_window/reset_operator_password）经 `AuditEvent`/`insert_diesel` 治理路径与领域 mutation 同事务写入；词汇与 §3.6.4 注册表逐字对齐由测试钉死（2026-08-15 变异探针实证：篡改 action_kind 3 项测试失败）。（2026-08-15 后新增的 `create_import_candidate` / `expire_import_candidate` 写入器晚于该 run，其证据归 G2。）
- 条目 6：`ci-contracts` lane（export-openapi → spectral → api:generate → `git diff --exit-code` → diesel schema golden）全绿。
- 条目 7：policy scan `module-dependency-scan: ok` 于同 run 真实执行；application/db 单向不变量由编译边界承载（db 的 persisted-fact 类型与 store error enum 对 `db` 私有，向上引用是编译错误）。

## 已登记待办（不阻断 gate，登记备查）

- **sibling 写入器 session DELETE 作用域覆盖**（2026-08-15，opus 审查外溢观察）：terminate/expire 路径的 session 删除若失去作用域过滤，现有单 operator fixture 测不出——reset 路径已由双 operator 隔离测试封堵（`bda92da`），terminate/expire 需要等价的多 operator 负向测试。归条目 2 的硬化项。
- **Web hook 层单测**（2026-08-15）：`useSession`/`useLogin`/`useLogout`/守卫三态目前由 Playwright e2e 行为覆盖；组件级单测需 jsdom + @testing-library，暂缓至有真实回归需求时引入。
- **Web 深链保留**（2026-08-15，审查 Low 项）：会话过期后登录不回原深链（`path="*"` replace 到 `/`）。体验项，随 Panel 功能页增多时评估。
- **reset-operator-password 真机 TTY 验证**（2026-08-15）：单测全覆盖；本地 dev 后端上的交互式实跑（改密→浏览器新密码登录闭环）待 owner 时间，非阻断。
- **`record_type` 封闭枚举强制**（2026-08-15，WP1 审查项，归 Phase 2）：ADR-0037 已冻结 `account_credential` | `import_payload` 枚举，但 migration 列无 CHECK、Rust 侧仅字面量常量；WP2 落 `account_credential` 写入器时一并收敛强制方式。
- **diesel 生成列 Integer/BigInt 类型地雷**（2026-08-15，WP1 审查项，归 Phase 2）：`revision_counters.*` 与 candidate baseline 列生成为 `Integer`(i32)，现有读写用显式 BigInt cast 规避；后续直接使用生成类型的读者会静默截断（现实值域内不触发）。建议 diesel patch file 改列型；WP2 的 CAS 读写必须沿用显式 cast。
- **`InvalidPersistedFacts` 卡死态恢复**（2026-08-15，WP1 审查项，归 Phase 2）：pending candidate 与 payload vault row 计数不一致时上传路径 fail closed 且无解锁手段；WP2 的 discard 实现须能清理该态（或登记显式恢复 runbook）。
