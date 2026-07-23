# Natsume V2 Phase 1 详细实施计划：最小控制域与 Server 基础

> 架构基线：`Natsume_V2_Design_v2.5.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.2.md`  
> 计划版本：Phase Plan v1.0  
> 基准窗口：W4–W9  
> Gate：G1  
> 前置依赖：G0

---

## 1. 阶段使命与边界

建立无 Event、无运行时 phase、无 Team metadata 的权威 Server 事务模型和 Web 控制面基础。Phase 1 负责把所有核心事实、约束、秘密存储、审计和状态变更放入可恢复的 SQLite 事务中，但不连接真实 Device。

本阶段必须在数据库层固化 v2.5 的证书边界：

- Enrollment 只保存 Device CSR/SPKI；
- `gateway_certificate_requests` 是 `SYNC_STATE` 的子资源；
- AutomationPolicy 无独立 Device/Gateway cert toggle；
- Target State 不含 expected Gateway fingerprint；
- Gateway certificate 是执行结果和 Observed fact。

---

## 2. 前置输入

- G0 支持平台、PKI ownership、目录/user/unit 决策；
- SNAFU/error-code registry；
- OpenAPI/Proto/D-Bus generation；
- Server root key/vault cryptographic spike；
- SQL migration harness 与 package test VM。

---

## 3. 详细工作包

### P1.1 SQLite schema 与 migration

实现：

```text
site_identity
instance_state
server_vault_records
system_configuration_revisions
automation_policy_revisions
seats
accounts
credential_revisions
seat_assignments
devices
device_bindings
device_certificates
gateway_certificates
device_target_states
observed_device_states
enrollment_challenges
enrollment_requests
csv_imports / csv_import_rows
operations / operation_targets
commands / command_attempts
gateway_certificate_requests
idempotency_records
audit_events / change_events
```

关键结构：

- `enrollment_requests` 只有 `device_csr_der`、`device_spki_sha256`；
- `gateway_certificates` 记录 `dns_san`、`certificate_profile_id`、`issued_for_configuration_revision_id`；
- `gateway_certificate_requests` 绑定 `device_pk`、`command_id`、`target_generation`、`configuration_revision_id`、CSR、SPKI、state、issued certificate；
- `automation_policy_revisions` 只有 auto approve enrollment/binding、auto sync state、auto prompt；
- Target snapshot 存 hostname/profile/minimum validity，不存 expected certificate fingerprint。

约束：Seat universe freeze、Seat immutable、username unique、active assignment/binding unique、Machine ID immutable unique、active cert unique、Command/idempotency unique、Gateway request/command/device/generation 一致性由事务服务与 DB 共同保证。

### P1.2 Server encrypted vault

- 创建 `/var/lib/natsume-server/keys/server-root.key`：CSPRNG、O_EXCL、O_NOFOLLOW、0400、fsync；
- HKDF/per-record AEAD/AAD/key-check；
- record types：password、staged password、Device CA key、Origin Intermediate key、Server control key、secret command payload；
- SQLite/WAL/temp/plaintext canary 扫描；
- root key 与 DB 分离 backup/restore 原型；
- key/AAD version 预留 migration。

### P1.3 Domain/Application services

- Seat、Account、CredentialRevision、SeatAssignment；
- Device、Binding、delete preconditions、no merge/split；
- SystemConfigurationRevision；
- AutomationPolicy；
- DeviceTargetState pure calculator；
- certificate requirement calculator；
- domain mutation 只更新 Server truth，不自动 dispatch。

### P1.4 Enrollment 与 certificate domain skeleton

- Enrollment challenge/request/poll state machine；
- approval result 语义为 Device certificate issuance；
- Gateway certificate request repository/service interface，仅接受 command context；
- certificate profile objects 与 post-sign parser interface；
- Phase 1 使用 test signer，不接真实 Client。

### P1.5 Operation/Command skeleton

- Operation、Target、Command、Attempt 状态机；
- frozen selection digest、deadline、offline policy；
- `SYNC_STATE` payload 非秘密；
- `SYNC_SECRET` payload 只能指向 Server vault record；
- fake online registry/dispatcher；
- Gateway request 必须引用 kind=`SYNC_STATE` Command。

### P1.6 HTTP/Auth/RBAC

- local operator accounts + Argon2id；
- secure cookie、CSRF、idle/absolute timeout；
- viewer/operator/lead/admin；
- re-auth：secret、PKI、delete、automation；
- Problem Details、ETag/If-Match、Idempotency-Key、cursor pagination；
- Enrollment approval API 描述明确为 Device certificate。

### P1.7 Audit、Change 与 SSE

- domain/Audit/Change 同事务；
- persisted cursor、Last-Event-ID、retention/reset；
- no password/raw hardware/CSR DER/private key；
- certificate audit 区分 `device_enrollment_certificate_issued` 和 `gateway_certificate_issued_for_sync_state`；
- JSONL export skeleton。

### P1.8 Web Shell

- route/layout/auth；
- Seat/Account/Device/Binding/Configuration/Automation/Operation/Audit；
- Enrollment 页面只有 Device CSR fingerprint；
- Gateway certificate status 独立显示，不提供 Enrollment 签发按钮；
- 2,000-row table virtualization baseline；
- generated API types。

---

## 4. 实施顺序

### W4–W5：Schema、Vault、Auth Skeleton

- 初始 migration 与约束测试；
- Server root key/vault；
- operator auth/session；
- OpenAPI skeleton。

### W6–W7：Domain、Target、Audit/SSE

- Seat/Account/Assignment/Device/Binding；
- Configuration/Automation；
- pure target calculator；
- Audit/Change/SSE；
- Web domain views。

### W8：Enrollment/Command Skeleton

- Device-only Enrollment state machine；
- Gateway request persistence contract；
- fake registry/Operation/Command；
- certificate UI state。

### W9：整合与 Gate

- crash/concurrency/security tests；
- backup/restart demo；
- Web → API → SQLite → Audit/SSE vertical demo；
- G1 evidence。

---

## 5. 交付物

- 初始 SQL migration 与 migration test suite；
- Server vault module + backup prototype；
- domain/application service modules；
- Auth/RBAC/API/OpenAPI；
- persisted SSE；
- Web Shell；
- fake Device/Operation harness；
- certificate lifecycle schema tests；
- G1 evidence bundle。

---

## 6. 验证矩阵

| 场景 | 预期 |
|---|---|
| 更新 Seat label | DB 拒绝 `seat_label_immutable` |
| 第二个 active DeviceBinding | unique constraint/typed conflict |
| Machine ID update | DB 拒绝 |
| Enrollment insert 含 Gateway CSR 字段 | schema 不存在/contract test 失败 |
| Gateway request 引用非 SYNC_STATE Command | service 拒绝稳定错误码 |
| Automation policy 试图设置 auto issue cert | OpenAPI/DB/UI 均无字段 |
| Target hash 输入 certificate fingerprint | pure calculator test 失败 |
| password canary | 只在 vault ciphertext，API/log/WAL scan clean |
| 同 Idempotency-Key 不同 body | conflict |
| SSE 断线重连 | cursor 补齐或 snapshot reset |

---

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| Domain service 绕过 DB constraint | 所有关键约束同时有 DB test 与 service test |
| Vault record 明文落 WAL/temp | canary test、短事务、禁止 plaintext column、temp-dir scan |
| Gateway request 过早做成通用 API | 无公开 HTTP route；repository 要求 command context |
| Web 误导 Enrollment 已完成 Gateway | 独立状态标签和 E2E negative assertion |
| Target hash 不稳定 | canonical serialization/golden fixture/property test |

---

## 8. G1 Gate 清单

- [ ] 完整最小 domain CRUD/constraints；
- [ ] Server vault key-check、tamper、restart、backup prototype；
- [ ] Enrollment schema 只有 Device CSR；
- [ ] Gateway request schema 与 active SYNC_STATE 绑定；
- [ ] Automation 无 cert issuance toggle；
- [ ] Target 无 expected Gateway fingerprint；
- [ ] password internal round-trip only；
- [ ] Web → API → SQLite → Audit/SSE demo；
- [ ] domain mutation 无 Device side effect；
- [ ] 无 Event/phase/Team metadata/merge-split；
- [ ] G1 evidence 已签署。
