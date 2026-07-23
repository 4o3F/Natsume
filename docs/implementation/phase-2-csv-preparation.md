# Natsume V2 Phase 2 详细实施计划：单 CSV 与 Preparation Center

> 架构基线：`Natsume_V2_Design_v2.5.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.2.md`  
> 计划版本：Phase Plan v1.0  
> 基准窗口：W7–W13，与 Phase 3 并行  
> Gate：G2A  
> 前置依赖：Phase 1 的 Domain、Vault、Auth/API、Target calculator 可用

---

## 1. 阶段使命与边界

完成当前实例唯一的数据入口：一个固定 `seat,account,password` CSV。导入只更新 Server truth、CredentialRevision、SeatAssignment 和非秘密 Target/Drift，不产生任何隐式 Device 副作用。

Preparation Center 必须向 operator 清楚表达：

```text
CSV/Binding 变化
→ Target drift
→ 显式 SYNC_STATE（其中按需签发 Gateway certificate）
→ Secret drift
→ 人工 SYNC_SECRET
→ Gateway READY
```

本阶段只实现 UI/Server 视角，不要求真实 Device executor。

---

## 2. 详细工作包

### P2.1 固定 CSV contract

- Header 严格为 `seat,account,password`；
- UTF-8 或 UTF-8 BOM；
- delimiter 只允许逗号；
- 首行为 header；禁止额外列；
- streaming parser；
- 文件大小、行数、字段数、字段长度、处理 deadline；
- `account`/`password` 必须同时有值或同时为空；
- canonical Seat/Account normalization；
- password policy 只返回结果，不回显值。

### P2.2 单 Import staging

- 每个 `CsvImport` 恰好一个 source；
- content hash、row count、seat-set hash、TTL；
- password 立即转入 Server encrypted vault staging record；
- 原始文件不长期保存；
- restart/expiry cleanup；
- 无 ImportSource、多文件 join、column mapping、XLSX/ODS/legacy encoding。

### P2.3 Preview

逐行状态：

```text
create_seat
assign
reassign
password_update
unassign
unchanged
error
```

Preview 只显示：Seat、Account、password present/changed、validation code、planned action。不得返回 password、长度、hash 或可用于猜测内容的信息。

### P2.4 Commit 与重复导入

首次 commit：

- 每个 Seat 恰好一次；
- 建立并冻结 Seat universe/hash；
- 创建 Account/Credential/Assignment；
- all-or-nothing transaction。

后续 commit：

- Seat 集合必须完全相同；
- unknown/missing/rename/duplicate Seat 阻止；
- account 变化 = reassignment；
- account 不变/password 变化 = new CredentialRevision；
- 两者不变 = no-op；
- 空 account/password = unassign；
- target generation 只因非秘密 assignment/config 变化而更新；password-only change 只产生 secret drift；
- 不创建 `SYNC_STATE` 或 `SYNC_SECRET`。

### P2.5 非秘密导出

- Seat/Account assignment，无 password；
- Device/Binding inventory；
- Device/Gateway certificate metadata/status；
- Operation/Fleet readiness；
- CSV formula injection hardening；
- 禁止 DOMjudge credential file、private key、CSR DER、Caddy runtime JSON。

### P2.6 Preparation Center

卡片/表格：

- Import revision/impact；
- Enrollment pending/conflict；
- Device certificate coverage；
- Gateway certificate status：`not_requested/requesting/active/invalid/failed`；
- Binding；
- target/applied generation drift；
- secret absent/stale；
- Gateway/Session/Home；
- recent Operations；
- readiness blockers。

操作入口分离：

- Approve Enrollment；
- Open Binding Prompt；
- Bind/Unbind；
- `SYNC_STATE`，预览 Gateway cert action=`reuse|issue|reissue`；
- `SYNC_SECRET`，独立 re-auth/reason；
- 无独立“Enrollment 签发 Gateway 证书”按钮。

### P2.7 Automation Policy UI

只包含：

- auto approve enrollment；
- auto approve binding；
- auto sync state after binding；
- auto open prompt；
- scope/limit/quality/expiry。

不存在 auto issue Device cert、auto issue Gateway cert、auto sync secret。UI 解释：Enrollment approval 自带 Device cert；Gateway cert 是 SYNC_STATE 的必要子步骤。

---

## 3. 实施顺序

### W7–W8

- parser/limits/normalization；
- staging vault records；
- fixture/canary corpus。

### W9–W10

- preview/commit/reimport transaction；
- Seat universe freeze；
- target/secret drift calculation。

### W11

- non-secret exports；
- formula injection；
- cleanup/recovery。

### W12

- Preparation Center；
- Automation UI；
- sync action previews/certificate status。

### W13

- Playwright、2,000-row performance、concurrent import tests；
- operator UAT；
- G2A review。

---

## 4. 交付物

- fixed CSV schema/template；
- parser/staging/preview/commit services；
- import migrations/TTL cleanup；
- non-secret export services；
- Preparation Center；
- Automation Policy UI；
- Playwright import workflow；
- operator import/reimport runbook；
- G2A evidence bundle。

---

## 5. 验证矩阵

| 场景 | 预期 |
|---|---|
| UTF-16/XLSX/semicolon/TSV | 拒绝固定错误码 |
| BOM UTF-8 | 接受 |
| duplicate/missing/unknown Seat | commit 阻止 |
| password-only change | CredentialRevision + secret drift，target generation 不变 |
| account reassignment | assignment revision + target drift |
| identical import | no-op，仍有 import audit |
| import commit crash | 全部提交或全部不提交 |
| staging expiry/restart | ciphertext 清理，无 plaintext |
| export 单元格以 `=+-@` 开头 | 安全前缀/转义 |
| UI | 不显示密码，不显示 Gateway Enrollment action |
| import commit 后抓包 | 无 Device network side effect |

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 运营人员把缺行当 unassign | 后续文件必须完整 Seat 集合，缺行阻止 |
| password 泄漏到 preview/WAL | staging AEAD + canary scan + no plaintext DTO |
| Preparation Center 把 state/secret 合并 | 独立状态列、独立动作、E2E assertions |
| 重新引入多文件/Excel scope | dependency/schema/route bans |
| 自动化误发密码 | Policy schema 根本不存在该字段 |

---

## 7. G2A Gate 清单

- [ ] one-file fixed CSV contract 完整；
- [ ] 首次 Seat freeze 与后续 exact-set 规则通过；
- [ ] reassign/password update/unassign/no-op 正确；
- [ ] commit all-or-nothing；
- [ ] password write-only、canary scan clean；
- [ ] non-secret exports only；
- [ ] import 不产生 Device side effect；
- [ ] Preparation Center 区分 target/secret/Gateway certificate 状态；
- [ ] Automation 无 cert/secret 独立签发开关；
- [ ] operator UAT 签收；
- [ ] G2A evidence 已签署。
