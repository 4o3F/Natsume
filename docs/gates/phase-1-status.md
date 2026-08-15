# Phase 1 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-15
> G1：`OPEN`（0/7 PASS）

Phase 1（Control Domain）交付面已全部落地，本文件手写追踪 G1 证据收敛。条目通过需可定位 evidence（CI run / commit / artifact 链接 + 一行结论 + 日期），不得以文档存在、scaffold 或截图替代可复现结果；partial pass 记为未通过。

交付面锚点（实现完成、证据待登记）：Stage 5B operator surface（current-fact migration、operator auth、audit、operator API/OpenAPI）；Web navigation 与 auth shell（轮询，`d73674f`）；单监听器同源托管面板（`d337469`）；`reset-operator-password`（`bda92da`）；session TTL 契约对齐（`b8efe48`）。

## G1 条目（7 项）

| # | 条目 | 状态 |
|---|---|---|
| 1 | migration：空库首装幂等；升级路径 | `OPEN` |
| 2 | 领域不变量与事务原子性（bootstrap/session/reset/device lifecycle 单事务与回滚负向测试） | `OPEN` |
| 3 | secret redaction（password/PHC/login name 不入日志与审计；错误 Display 泛化；canary 断言） | `OPEN` |
| 4 | 两角色授权（admin/viewer 已挂载 route 全矩阵，viewer→admin action 稳定 `403`） | `OPEN` |
| 5 | audit 原子性与词汇注册表对齐（与领域 mutation 同事务写入；写入器词汇与 §3.6.4 注册表逐字一致） | `OPEN` |
| 6 | API generated clean diff（OpenAPI/TS/diesel golden，`ci-contracts` lane） | `OPEN` |
| 7 | 模块依赖扫描（application/db 单向不变量、policy scan `module-dependency-scan`） | `OPEN` |

## 证据登记要求

- 条目 1–7 的测试与扫描均已随 CI lane 运行；登记需要含全部 Phase 1 交付面（≥ `c5eb020`）的 head 上一次全绿 CI run 链接，逐条对应其 lane/测试集，另附各条已知限制。
- 条目 1 已知限制（预登记）：升级路径当前只能做 same-version reinstall 与空库→当前 schema 首装——V2 无已发布前版 schema，跨版本 migration 用例随首个发布版建立（与 G0 条目 12 同源限制）。
- 条目 2/5 已知限制（预登记）：reset 路径的审计失败回滚与双 operator 作用域隔离已有负向测试（`bda92da`）；sibling 写入器的等价负向覆盖见「已登记待办」。

## 已登记待办（不阻断 gate，登记备查）

- **sibling 写入器 session DELETE 作用域覆盖**（2026-08-15，opus 审查外溢观察）：terminate/expire 路径的 session 删除若失去作用域过滤，现有单 operator fixture 测不出——reset 路径已由双 operator 隔离测试封堵（`bda92da`），terminate/expire 需要等价的多 operator 负向测试。归条目 2 的硬化项。
- **Web hook 层单测**（2026-08-15）：`useSession`/`useLogin`/`useLogout`/守卫三态目前由 Playwright e2e 行为覆盖；组件级单测需 jsdom + @testing-library，暂缓至有真实回归需求时引入。
- **Web 深链保留**（2026-08-15，审查 Low 项）：会话过期后登录不回原深链（`path="*"` replace 到 `/`）。体验项，随 Panel 功能页增多时评估。
- **reset-operator-password 真机 TTY 验证**（2026-08-15）：单测全覆盖；本地 dev 后端上的交互式实跑（改密→浏览器新密码登录闭环）待 owner 时间，非阻断。
