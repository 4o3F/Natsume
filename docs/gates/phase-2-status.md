# Phase 2 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-16
> G2：`OPEN`（8/10 PASS；条目 1–8 已锚定全绿 CI run，条目 9/10 的补强测试已实现、待其 head 的全绿 run 登记后关门）

Phase 2（CSV Preparation）交付面已全部落地，本文件手写追踪 G2 证据收敛。条目通过需可定位 evidence（CI run / commit / artifact 链接 + 一行结论 + 日期）；partial pass 记为未通过。

交付面锚点：契约冻结 `3d03b68`；WP1 加密 staging 与权威 diff `85aa455`；WP2 原子 commit/discard `96287ff`；WP3 HTTP 面挂载 `6684315`；WP4 pending 读取面与 Preparation Center `bbf5fb8`。每包均经 opus 对抗审查（阻断项全数修复）、独立复验与变异探针（合计 8 针全部命中：过期删除、diff 分类、双 CAS、unbind 作用域、revision 双标志独立性、角色守卫、层序、读路径 lazy expiry）。

## G2 条目（10 项，主题引自 roadmap）

| # | 条目 | 状态 |
|---|---|---|
| 1 | malformed/duplicate/empty candidate 拒绝（解析矩阵 + 行号级脱敏错误） | `PASS` |
| 2 | 单 pending mutual exclusion（singleton + 409） | `PASS` |
| 3 | 非秘密维度 first/no-op/material 区分（revision 双标志独立性测试） | `PASS` |
| 4 | 已提交 import 无条件推进全部 `credential_revision` 且 preview 零密码变化分类 | `PASS` |
| 5 | 双 CAS 拒绝与重复提交安全失败（stale 保留 candidate、audit rejected） | `PASS` |
| 6 | candidate/payload 终态删除（commit/discard/lazy expiry 三路径 + 容忍恢复） | `PASS` |
| 7 | current-fact credential/mapping（账户按 username 存续、全量新 nonce 重封） | `PASS` |
| 8 | 事务回滚（重复 audit-ID 注入负向，expire+create 与 commit 两路径） | `PASS` |
| 9 | password 明文不进任何普通 surface（DB+WAL 字节扫描、响应/审计/日志 canary） | `OPEN` |
| 10 | CSV → Server truth 且零自动 Command（import 零 Device I/O） | `OPEN` |

## 已登记证据

- 条目 1–8：[ci run 31932403934](https://github.com/4o3F/Natsume/actions/runs/31932403934)（head `a544b96`，2026-08-16，全 lane 绿，含交付面锚点 `bbf5fb8` 的全部祖先提交）——各条目的具名测试（解析矩阵、singleton 409、revision 双标志三态、无条件 credential 推进与 preview 零密码分类、双 CAS stale-reject、三路径终态删除、nonce 全量重封、双路径回滚注入）随 ci-rust / ci-contracts / ci-web lane 真实运行全绿。（2026-08-16 勘误：此前「150 项 server 测试、12 条 Playwright」计数失真，证据以 run 内实际执行为准，不再登记手数数字。）
- 条目 9：字节扫描（DB+WAL）与响应/审计 canary 已在上述 run 内；**日志 canary 与 `-shm`/checkpoint 扫描于 2026-08-16 补齐**，锚定其 head 的全绿 run 后翻 `PASS`。
- 条目 10：import 快照新增 `commands`/`observed_device_states` 计数与成功 commit 路径零行断言（2026-08-16 补齐，Phase 4 WP1 已落地 `commands` 写入面，负向断言由结构性声明升级为真实测试）；锚定其 head 的全绿 run 后翻 `PASS`。

## 已登记待办（不阻断，登记备查）

- **canonical UUIDv7 guard 不校验 variant nibble**（2026-08-16，WP3 审查项）：import/contest/device 三处 guard 只查 version+往返；published schema 拒绝而服务端接受。归 Phase 3+ 硬化。
- **`GET /api/v2/imports` 轮询取 RESERVED 写锁**（2026-08-16，WP4 审查观察）：`pending: null` 常态每 10s 每 tab 一次 BEGIN IMMEDIATE；当前规模无害，若 operator tab 数上升可改「先 deferred 读、观察到过期再进写事务」。
- **首页落点**（2026-08-16）：导航首项为 Preparation 而 index 重定向仍为 /seats；有意保留，随 Panel 页面增多再定。
