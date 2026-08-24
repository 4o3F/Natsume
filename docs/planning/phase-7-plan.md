# Phase 7 执行计划：Production Release

> 状态：`DRAFT-PLAN`（2026-08-16 起草）
> 适用：Phase 7 启动时提升为 `docs/gates/phase-7-status.md` 的启动分解基线，届时按最新事实修订
> 权威来源：[路线图](../roadmap.md) §Phase 7 与 G7 覆盖、§6 证据标准、[安全与恢复](../security-recovery.md) §7–§10、[支持平台](../supported-platform.md)、[依赖策略](../dependency-policy.md)
> 前置：Phase 6 关闭；[路线图](../roadmap.md) External buffer +2–4 周用于外部依赖

本文件是计划，不是完成声明。Phase 7 的特征是**决策密度高于代码量**：多数交付物（发布流水线、备份/恢复、审计导出、PKI runbook）当前完全不存在，且强依赖 owner 的部署侧输入。

## 1. 阶段目标与边界

**结果**：production Deb 与可重复的发布流程、installation/preseed/reconfigure、upgrade/rollback、backup/restore、赛后审计导出、PKI 材料 runbook、observability 决策、500 台并发容量验证、operator/admin runbooks、readiness rehearsal、release artifacts 与 SBOM。

**证据标准**（[路线图](../roadmap.md) §6，Phase 7 尤其严格）：每条 = 链接 + 一行结论与日期；文件存在≠通过；截图不替代可复现日志；**VM 不能替代物理硬件 fixture**；scaffold 不替代实现；repo pin 不替代环境冻结；partial pass 不关闭整个 Gate；waiver 不放宽安全不变量。

## 2. 入场检查

| # | 检查项 | 依据 | 阻塞范围 |
|---|---|---|---|
| E1 | 硬件 fixture 实地采集完成（G0-IN-005 已降级为非 G0 阻塞，但[支持平台](../supported-platform.md)要求「首次 provisioning 前完成」） | 物理硬件 fixture 不可由 VM 替代 | 阻塞 rehearsal 与 G7 签署 |
| E2 | Server 目标 OS（Ubuntu 26）上的 lifecycle 证据（CI runner 与 packaging smoke 当前均 ubuntu-24.04；owner 假设向前兼容，但假设不替代证据） | [支持平台](../supported-platform.md) | 阻塞 WP2/WP9 |
| E3 | skia 预编译 archive 的 URL/release pin + SHA-256 + CI 校验 + 离线重建路径（[依赖策略](../dependency-policy.md)明写「形成目标发布证据前必须补充」） | 最大剩余供应链缺口 | 阻塞 WP8/WP10 |
| E4 | 时钟 skew 容差冻结（若 Phase 5 未闭） | Command deadline、证书窗口、UUIDv7 时序均静默依赖 | 阻塞 WP9 rehearsal 判据 |

## 3. 现状盘点（Phase 7 的起点）

### 3.1 已就绪

Client/Server 两个 nFPM 包（Client 声明 9 项 depends、4 个 `config|noreplace` conffile）、debconf 两问 preseed 与 `postinstall` 的规范化/原子写/三态分类、sysusers 与 tmpfiles（含 `/var/lib/natsume-server/backups` 目录）、Caddy 与 nFPM 的五组 pin + SHA-256 校验链、`hosted-lifecycle.sh`（含 G3 主题 16 的凭据保留断言）、`target-vm/phase0-lifecycle.sh`（含 reboot 与 V1 残留守卫）、三个 CI workflow（`ci` / `nightly` / `package-lifecycle`）。

### 3.2 完全缺失

发布流水线（无 tag 触发、无签名、无发布步骤、无长期 artifact 保留；版本硬编码 `2.0.0~ci1`）、备份/恢复（只有空目录）、审计导出、PKI ceremony runbook、observability 决策、SBOM、operator/admin runbooks、rollback 策略、site config 的签名投放机制。

### 3.3 已知缺陷（须在本阶段修）

| # | 缺陷 |
|---|---|
| F1 | 两个包注册**同三个 conffile**（`site.toml` + 两个 trust root），当前靠全 purge 规避；同机共装会冲突 |
| F2 | 两个 postinstall 只 `daemon-reload`，**无 `systemctl enable`/preset 激活**——单元安装后惰性 |
| F3 | Server 的 `config.toml` **不是 conffile**（经通用目录树复制），升级静默覆盖 operator 修改，而契约称其为「唯一配置源」 |
| F4 | Server 包**无任何 `depends:`**（至少 `systemd`、`ca-certificates`） |
| F5 | `justfile` 的 `verify` 聚合不含 `ci-packages` 与 `integration`；`package-*` recipe 依赖调用方预导出 6 个变量，裸跑会在 nfpm 配置留下未解析 `${…}` |
| F6 | 占位物未替换：`client.preseed` 用 RFC 5737 文档段 IP、`site-config.example.toml` 三处 `REPLACE-WITH-…`、`caddy.sha256.example` 为零引用死文件、home-templates 与 browser-policy 仅占位 README |

## 4. 工作包分解（候选基线）

### WP1：包拓扑修复（F1–F4）

- 目标：conffile 冲突定案（拆共享包 / `conflicts` / 明确禁止同机共装）；单元激活方式定案（postinstall `enable` vs 镜像 preset vs runbook 手工）；Server `config.toml` 转 `config|noreplace` 或明确宣告不可本地修改；Server 包补 `depends`。
- 测试：同机共装/拒绝的 lifecycle 用例；升级保留 operator 修改的配置断言；首启后单元处于预期激活状态的断言。

### WP2：发布流水线与版本方案

- 目标：tag 触发的 release workflow，产出带正式版本号的双 deb + artifact 长期保留 + 校验和；`justfile` 补 `release` 相关 recipe 并修 F5。
- 冻结项：版本号方案（语义化 vs 日期）、artifact 签名方式（**D1**：是否 GPG 签 deb / 仅发布 SHA-256 清单）、site config 的 signed input 投放机制（**D2**）。

### WP3：备份与恢复

- 目标：备份对象、一致性方法、restore-to-verified-state 流程与验证判据；runbook + 集成测试（[依赖策略](../dependency-policy.md)要求 backup/restore 由 runbook 与 integration test 双重验证）。
- 冻结项：备份对象清单（SQLite DB + WAL、vault 主密钥、origin CA 材料、site.toml、TLS leaf/key）；一致性方法（`VACUUM INTO` / `.backup` / 停服快照，**D3**）；**vault 主密钥是否与 DB 分离保管**（主密钥丢失等同 vault 数据不可恢复，**D4**）；restore 后的验证判据（Observed/Drift/certificate inspection/audit 四路复核）。
- 约束：[安全与恢复](../security-recovery.md)恢复原则——先存证据再改状态；不把删除凭据/identity/本地状态文件/DB 行当首选修复；**每个 destructive step 必须有备份/rollback 条件**；任何身份重建、凭据替换、Device replacement 与 contest reset 必须人工明确授权。

### WP4：赛后审计导出

- 目标：导出格式、脱敏规则、触发方式、访问控制。
- 冻结项：是否新增 HTTP route（**D5**）；导出粒度与时间范围；格式（JSONL / CSV）。
- 约束：只含 typed allowlisted evidence——**不含** password / private key / token 值 / 原始 CSV / ciphertext / CSR / 证书正文 / 完整路径 / 未脱敏 source chain / `request_fingerprint_sha256`；ADR-0031 F9：审计仅面向赛事管理员，不对外提交。
- 测试：导出内容的禁入项字节扫描；`enrollment_requests` 与 `commands` 终态行的清理与导出（Phase 7 明确归属项）。

### WP5：PKI ceremony runbook

- 目标：两根 CA 的生成/保管/签发/投放/轮换/销毁全流程文档 + 可验证检查点。
- 内容（已冻结的约束）：control root 自签、**私钥离线保存、运行中 Server 不得持有**、签发 Server TLS leaf 且 **IP-SAN 必须等于部署实际地址**、leaf/key 以 DER 放入私有状态目录（权限不宽于 `0600`、目录不宽于 `0700`）；origin CA 私钥置于 `/var/lib/natsume-server/keys/origin-ca.der` + `origin-ca-key.pk8`，`serve` 在 bind 前校验并与 packaged PEM 做**逐字节 equality preflight**；`bootstrap` 与 `reset-operator-password` 绝不创建/修改/校验这两个文件。
- 冻结项：制包期如何保证 `local-origin-root` 打包 PEM 与 `origin-ca.der` 的逐字节一致（当前 preflight 只在 `serve` 时兜底，**D6**）；保管人与介质；轮换路径。

### WP6：Gateway validity 赛前校验工具化

- 目标：赛前对全 fleet 已签发证书的 `not_after_unix_ms` 做一次可复现检查并呈现结果。
- 现状：签发时与 site startup preflight 已校验（`GATEWAY_MINIMUM_REMAINING_VALIDITY_SECONDS = 300`、`gateway_not_after ≥ contest_end + 86400`），但**无 operator 查询面**（`gateway_certificates.not_after_unix_ms` 为 INTEGER UTC epoch milliseconds）。
- 冻结项：谁在赛前跑、检查项、结果呈现位置（**D7**：新 route / CLI 子命令 / Panel 视图）。

### WP7：500 台并发容量验证

- 目标：G7 的完整容量验证（缩比探针归 G4）。
- 冻结项：真实 500 台 vs 模拟客户端（**D8**）；场地与时段；SQLite 单写者路径的判据阈值；Observed 上报速率假设；失败后的回退设计选项。
- 参考：`MAX_CONCURRENT_CONNECTIONS = 2_048` 由「500 台 + 重连风暴 + 约 3 operator browser」推得（约 4× 余量）。

### WP8：供应链收口与 SBOM

- 目标：关闭 E3 的 skia archive 缺口；产出 SBOM 并纳入 release artifact；Caddy 升级为 `ENV-FROZEN`（source、archive checksum、binary checksum、module closure、目标 OS 执行、package lifecycle 全部签收）。
- 冻结项：SBOM 格式（CycloneDX / SPDX，**D9**）与生成时机；skia archive 的 pin 与离线重建路径（**不得延伸为安装期/首启/运行时下载**）。

### WP9：Rehearsal 与 runbooks

- 目标：clean-site rehearsal（含失败与恢复场景、restore to verified state、offline steady state）+ operator/admin runbook 集合。
- runbook 至少覆盖 9 个流程：bootstrap 首启（TTY）、`reset-operator-password`、provisioning 窗口开关、Enrollment 审批/拒绝、CSV import commit/discard、`SYNC_STATE`/`SYNC_SECRET` 触发、Home reset（operator-present）、备份/恢复、单生命周期竞赛重置（破坏性，删除 pending candidate 后下一次 import 走普通 first-import lifecycle）。
- 冻结项：rehearsal 场地/机器数/时长、签署人与签署文档位置（**D10**）。

### WP10：Observability 决策

- 目标：显式定案——引入 metrics 端点/后端，还是明确「不做 metrics，只靠 Observed + audit + journald」。
- 现状：Server 只写 stderr 由 journald 收集，配置只有 `[log].level` 封闭枚举；[状态与执行模型](../state-and-execution.md) §8 只给了指标 label 禁止清单（不得含 password、token 值、路径、certificate body、Machine ID 全值或自由格式错误）。
- 约束：Device 侧带宽受限（ADR-0030 F2），指标汇聚方案须与之相容。

### WP11：Upgrade / rollback 与 migration 纪律

- 目标：定案回滚策略并建立首个发布版后的增量 migration 纪律。
- 冻结项：是否禁止降级；回滚是否依赖「保留前版 deb + DB 备份还原」；**预发布单一 migration 策略在首发后失效**，增量迁移纪律须在本阶段冻结（**D11**）。
- 约束：[契约](../contracts.md) §13——Phase 7 发布签收的 descriptor 才建立 field-number 兼容基线；在此之前 active-development Proto 删除字段不留 `reserved`、编号按当前结构整理。发布后 field number / interface name / method / ID 与 revision 语义不复用，破坏性 wire 变化使用新 WS subprotocol 或 interface version；**不假设 schema 自动回滚**。

## 5. G7 覆盖项 → WP 映射

| G7 主题 | WP |
|---|---|
| clean-site rehearsal | WP9 |
| 失败与恢复场景 | WP3 + WP9 |
| restore to verified state | WP3 |
| package lifecycle | WP1 + WP2 |
| offline steady state | WP9 |
| 500 台并发容量 | WP7 |
| audit export | WP4 |
| Gateway validity 赛前校验 | WP6 |
| release decision 签署 | WP9（签署形态见 D10） |

## 6. owner 决策点

| # | 决策 |
|---|---|
| D1 | release artifact 签名方式（GPG 签 deb vs 仅 SHA-256 清单） |
| D2 | site config 的 signed input 投放机制（`site-config.example.toml` 已声明该机制但不存在） |
| D3 | 备份一致性方法 |
| D4 | vault 主密钥是否与 DB 分离保管 |
| D5 | 审计导出是否新增 HTTP route |
| D6 | 制包期保证 packaged PEM 与 `origin-ca.der` 逐字节一致的机制 |
| D7 | Gateway validity 赛前校验的呈现面 |
| D8 | 500 台容量验证用真实机群还是模拟客户端 |
| D9 | SBOM 格式与生成时机 |
| D10 | rehearsal 规格与 release 签署形态/位置 |
| D11 | 首发后的增量 migration 纪律与降级策略 |
| D12 | conffile 冲突与单元激活方式（WP1 的两项） |

## 7. 跨切风险

| 风险 | 控制 |
|---|---|
| 硬件 fixture 与目标 OS 证据晚到 | E1/E2 前置；External buffer +2–4 周 |
| skia archive 无法离线重建 | E3 先做可行性验证；不可行则需 GUI 依赖方案回退（影响 Phase 6 产物） |
| 备份/恢复设计过晚导致 rehearsal 无法覆盖 | WP3 排在 WP9 之前 |
| 发布签名与 site config 投放机制并行缺失 | D1/D2 合并定案，二者共用签名基础设施 |
| 首发后 migration 纪律未定即发版 | D11 必须先于 WP2 的首个正式 tag |
