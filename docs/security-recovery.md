# Natsume V2 安全与恢复不变量

> 状态：`NORMATIVE`  
> 适用范围：所有进程、协议、持久化、打包、运维和恢复  
> 规则：放宽任何 `INV-*` 必须先有已接受 ADR  
> 校准基线：[ADR-0030](adr/0030-foundation-deployment-and-delivery-baseline.md) 的部署事实与信任假设

本文件定义少量稳定不变量。字段级负向案例属于 contract tests、policy scan 和 runbook，不在架构正文中重复枚举。

## 1. 威胁模型

Natsume 主要防护：

- 未认证或错误 Device 获取控制面权限；
- provisioning 窗口之外出现任何签发路径；
- 网络输入扩展成任意本地特权；
- 密码通过 API、日志、UI 或错误泄漏；
- Machine ID 改变、复制或不可用时误用旧凭据；
- 陈旧 Command/revision/epoch 操作当前状态；
- 未验证证书或配置进入 Caddy READY；
- LAN 上的假 Server 投喂命令或收割 Enrollment；
- Session Agent 获得过多权限；
- 恢复操作通过删除证据"修好"故障；
- 文档或 Gate 在无证据时宣称安全已验证。

不承诺防护（ADR-0030 T2）：

- 本地 root；
- 物理访问和固件篡改；
- 已被完全控制的 Server；
- DOMjudge 自身漏洞；
- 竞赛网络之外的通用终端安全；
- Server HA；
- Session lock 作为网络隔离；遮罩类 UI 作为完整性边界。

## 2. 信任与秘密清单

| 资产 | 保存位置 | 可使用者 | 禁止消费者 |
|---|---|---|---|
| Server control private key | Server 受限文件/approved secret store | Server TLS adapter | Web、Device、Helper |
| Offline control root key | 离线介质 | PKI ceremony | 运行中 Server |
| Device Token | Server DB（仅哈希）；Client `0600` 凭据文件 | WSS 认证 adapter | Agent、Helper、Caddy、浏览器 |
| Gateway private key | Client `0640 root:natsume-gateway` 文件 | Caddy | Agent、Helper、Server |
| DOMjudge password | Server vault（每个 subject 仅当前 ciphertext）；Client `0600` 凭据文件；渲染的 Caddy `/login` 注入配置（`0640`） | secret sync、Daemon 渲染、Caddy `/login` header 注入 | API、Observed、日志、Agent、状态页 |
| Fleet namespace UUID | 配置/Server truth | identity derivation | 无秘密属性 |
| Machine Hardware ID | identity file/Server metadata | identity、Enrollment | 不作为密码或 token |
| Operator session | Server/Web secure session | operator API | Device control |

秘密必须有明确 owner、生命周期、存储、使用者和销毁路径。不存在"暂时放到普通配置"这一例外。含凭据的 Caddy 配置是 secret artifact，与凭据文件同级管理（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）。

## 3. 核心不变量

### `INV-SECRET-01`：秘密不可观察

password 明文、private key、Device Token 值**不得**进入：

- 通用 HTTP response；
- Web 持久化状态；
- Target；
- Observed；
- Audit diff；
- 普通导出；
- 日志和指标；
- Session Agent；
- Helper；
- Caddy 状态页；
- ErrorCode detail 或 source chain。

允许的秘密路径必须使用专用类型、最短生命周期和 redacted 结果。Import 普通边界只使用 opaque `preview_token` 与 redacted 的**非秘密**分类/计数/identity/revision 证据（密码内容本身不产生任何分类证据，见 [ADR-0031](adr/0031-contest-import-and-secret-evidence.md)）。

按 [ADR-0031](adr/0031-contest-import-and-secret-evidence.md)：password-derived digest/length/fingerprint 不再作为独立禁止类别维护（审计仅面向内部管理员，F9）；工程默认仍不输出这些值。F9 失效时该条必须重审。

**验证入口：** secret scan、API/schema snapshot、日志测试、UI/e2e、Command redaction、package scan。

### `INV-INPUT-01`：网络输入只能映射到封闭契约

网络输入**不得**成为任意：

- shell/argv；
- 文件路径；
- UID/GID；
- systemd unit；
- 环境变量；
- URL/upstream；
- Caddy fragment；
- certificate profile；
- D-Bus method name；
- SQL fragment。

路径、UID、hostname、upstream 和 capability 必须由本地/Server policy 派生或 allowlist 校验。

**验证入口：** schema、property test、policy scan、Helper/D-Bus tests、负向 integration tests。

### `INV-IDENTITY-01`：Machine Hardware ID 不是认证凭据

Machine Hardware ID 是站点 namespace 下的稳定标识（派生配方见 [ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md)），不是 secret、token 或 certificate。网络认证必须使用 Device Token、TLS 和 operator auth。

原始硬件 serial 只在最小采集边界短暂出现；日志、Server API 和 fixture 只保存规范化/匿名化结果。

**验证入口：** library tests、helper boundary、fixture review、log scan。

### `INV-IDENTITY-02`：身份检查先于凭据使用

Device 启动必须先验证当前 Machine Hardware ID，再读取或使用任何 identity-bound 产物（Device Token、Gateway key、Seat 凭据、LKG）。

当 identity-bound 产物已存在且当前身份不可获得、与持久化值不匹配或有效来源不足（少于 2 个，见 ADR-0032）时，Device 必须 fail closed。凭据文件损坏同样 fail closed，不能自动重建或自动 re-enroll。

**验证入口：** startup decision table、fault tests、configured-disk-copy fixture、recovery runbook。

### `INV-CERT-01`：签发面在窗口内封闭

签发路径固定为两段：

```text
server-auth TLS（全部入口，预置 trust + IP-SAN 验证）
  → provisioning 窗口内 Enrollment：签发 { Device Token + Gateway certificate }
  → WSS 控制面（token + resolved Device state=`enrolled` 认证）；SYNC_STATE / SYNC_SECRET 不签发任何东西
```

窗口关闭时不存在任何签发路径。`provisioning_window` 是一个当前 singleton，只含 `state`（`open`/`closed`）、单调 `revision` 与 `last_audit_event_id`；正常 open/close 与其 redacted audit 以同一 transaction 的 guarded operation CAS 提交；AuditEvent 是证据历史，不保留逐次 provisioning revision 状态行。restart/restore 绝不自动开启：已 `closed` 时零写入，只有已 `open` 时才以 `system:recovery` audit 原子 close 并将 revision 加一；成功后再次恢复不产生第二条 close audit（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）。Enrollment 之外、operator API 与任何 Command 都不能获得 token 或证书。

`revision` 为 64 位有符号整数；溢出时恢复失败、启动 fail closed，不回绕；db 层以显式 `BigInt` 绑定绕过 Diesel schema 的 `Integer` 渲染。

**验证入口：** 窗口关闭负向测试、OpenAPI/DB/schema tests、无 token upgrade 拒绝测试。

### `INV-CERT-02`：授权属性由 Server 派生

Gateway SAN、hostname、profile、EKU 和 validity 必须由 Server 站点配置与冻结 policy 派生。CSR 自报字段不授予权限，只证明 possession 与公钥结构。

同一 `MachineHardwareId` 窗口内重复 Enrollment 只有在既有 Device 当前为 `enrolled` 时才可成为受审计的替换：旧 token 失效、新产物签发。`disabled` / `revoked` Device 的 intake、live replay 与 approved claim 全部以 identity conflict 零写入拒绝，approval 也在 audit/CAS 前拒绝；重开窗口不构成 enable/reactivate 权限。

**验证入口：** CSR SAN ignore test、替换语义与审计测试、certificate inspection。

### `INV-STATE-01`：非秘密状态显式应用

Target 只含非秘密数据且本身惰性。CSV、binding 或配置变化不得自动产生远端副作用。

只有人工发起的 `SYNC_STATE` 可以应用 Target。Observed 是实际状态来源，Drift 是纯比较。

Import 不创建 Command，不自动 `SYNC_STATE` 或 `SYNC_SECRET`，也不表示 Device 已同步。失败、discard、过期、stale baseline、binding-stale 与 transaction rollback **均不得**改变 confirmed configuration、binding、Target truth 或相关 revision。

**验证入口：** domain tests、UI authorization、offline Device tests、import non-mutation tests。

### `INV-SECRET-02`：秘密只由人类明确同步

没有自动 `SYNC_SECRET`。Import Commit、binding 变更或 confirmed contest configuration 替换本身**不得**创建或暗示 secret sync。

每次 secret sync 必须：

- 由授权操作员明确发起；
- 绑定当前 Device、Seat、BindingRevision 和 credential revision；
- 使用 durable Command；
- 在 Device 写入前再次校验；
- 返回 redacted 结果；
- 可审计。

**验证入口：** authorization tests、stale revision tests、secret scan、audit tests。

### `INV-COMMAND-01`：Command durable、identity-stable 且不重复副作用

Panel 在创建前生成 canonical lowercase hyphenated UUIDv7 `command_id`，并通过 `PUT /api/v2/commands/{command_id}` 提交。Server 先持久化 Command 与创建 audit 再投递；Device 先持久化 receipt/journal 再确认。相同 ID 必须原样贯穿 HTTP、WSS、journal、CommandStatus 和 audit correlation，且不得重复副作用。

Server 用 `request_fingerprint_version` 与 `request_fingerprint_sha256` 区分同 ID replay：相同 fingerprint 返回既有 Command；不同 fingerprint 返回 `COMMAND_REQUEST_CONFLICT`，不得覆写既有 Command。非 canonical UUIDv7 返回 `COMMAND_ID_INVALID`。Device journal 保存收到的 Command frame bytes；同 ID 但 frame bytes 不同必须以 `COMMAND_PAYLOAD_CONFLICT` 拒绝。Server 从已存储的 frozen payload 确定性渲染给定 ID 的 wire Command，使每次重新投递的 frame byte-identical；崩溃和重连后必须能恢复既有状态。每 Command 的 frozen content 只保存在 typed JSON，而不使用一组专用 top-level columns。

**验证入口：** UUIDv7 正/反例、`201/200/400/409` contract、same-ID fingerprint conflict、HTTP/WSS/journal/status/audit ID 一致性、crash/fault injection、duplicate delivery、journal durability、reconnect tests。

### `INV-PRIVILEGE-01`：最小权限

Privileged Helper：

- 无外网；
- 无秘密；
- 无任意执行；
- 只提供封闭 capability。

Session Agent：

- 无凭据/PKI/Caddy 所有权；
- 不访问 Server；
- 只处理 typed local snapshot/action；
- 只由 XDG Autostart 直接启动。

**验证入口：** D-Bus policy、package scan、network namespace/seccomp/AppArmor policy（如采用）、negative method tests。

### `INV-DATAPLANE-01`：数据面必须 fail closed

Caddy 只有在证书、私钥、SAN/profile、配置和本地健康检查全部验证后才能进入 READY。

否则保持已验证 LKG，或进入 BLOCKED/503。状态页只显示有限 allowlist 数据，绝不代理未验证 upstream。

**验证入口：** config validation、bad cert/key/SAN tests、reload rollback、status page security tests。

### `INV-DATAPLANE-02`：凭据注入只经 TLS upstream

DOMjudge 凭据只通过 Caddy 对 `/login` 路由的 header 注入进入数据面（[ADR-0034](adr/0034-state-execution-and-data-plane-boundary.md)）；**Caddy → DOMjudge upstream 必须为 TLS**，至少覆盖 `/login`。upstream 非 TLS 时不得激活注入配置。本机 loopback HTTPS 不替代该要求。

**验证入口：** 非 TLS upstream 拒绝激活测试、`/login` 之外路由无注入头的负向测试、含凭据配置的权限/日志脱敏检查。

### `INV-SESSION-01`：Session/Home 使用 epoch 且不拥有 Caddy

所有 Session/Home 动作绑定当前 epoch。陈旧 Agent、陈旧 UI action、陈旧 Home cleanup 必须拒绝。

Home 无法证明安全时不得启动受管 session。Session lock/unlock/terminate 不调用 Caddy、不改变 Caddy config/状态。遮罩类 UI（如未来实现）是呈现层，不是完整性边界；完整性依靠 `SESSION_TERMINATE` 与数据面 BLOCKED（[ADR-0035](adr/0035-session-home-and-desktop-cycle.md)）。

**验证入口：** 桌面 capability 清单、epoch race tests、Caddy call counter、Home fault recovery。

## 4. PKI 结构

两条平行的单层链，各自只被真正需要验证它的一方信任：

| 链 | 签发 | 信任方与分发 |
|---|---|---|
| control CA → Server TLS leaf | 离线生成，runbook 保管；Server leaf 经批准的离线流程签发 | Device 与操作员浏览器；经 package/debconf 预置 `control-ca.crt` |
| origin CA → 各 Device Gateway leaf | origin CA key 在 Server 上，provisioning 窗口内经 Enrollment 签发 | 各设备本机浏览器；origin CA 证书经包构建期注入 `local-origin-ca.crt` |

不存在 Device Identity CA（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）。每张 Gateway certificate 的 `gateway_certificates` row 只有 `certificate_id`、`device_pk`、`enrollment_request_id`、serial、SPKI hash、not-after、status；不存 certificate body，不建吊销分发机制；`revoked` / `retired` 状态行予以保留，用于撤销语义与审计回溯。

## 5. 秘密存储

### 5.1 Server vault

- 应用层 AEAD；
- `server_vault_records` row 只有 `vault_record_id`、`record_type`、`subject_id`、`nonce` 和 `ciphertext`；没有 format/key/AAD version、timestamp 或 rotation metadata；
- 每个 `(record_type, subject_id)` 只有一个当前 ciphertext。已提交的 Import Commit 无条件替换该 record 并推进对应 `credential_revision`，不建立 superseded、active/inactive 或历史 credential 行；
- DB 备份不应单独恢复出明文；
- key 不通过 argv、env、日志或 Web；
- secret read 只通过专用 use case；
- audit 记录访问、替换与终止动作但不记录值。

CSV / candidate import 的 password-bearing 材料只进入 encrypted staging 与 secret-safe persistence。全局只有一个 encrypted pending candidate；严格解析成功才写入。candidate row 存在即为 pending，不使用 workflow state/history。commit、discard 或 expiry 在同一事务删除 candidate 与其 payload vault record，并留下 redacted audit lineage；staging 失败、未成功 Import Commit 或终止候选不得把明文残留到普通 surface，也不得改变 confirmed contest configuration。

这里的删除是删除可寻址数据库事实，不承诺 SQLite page、WAL、backup 或底层介质上的取证级物理擦除。备份保留、介质销毁和 destructive reset 属于恢复 runbook，不得以“已经逻辑删除”替代其操作。

### 5.2 Client 凭据文件

按 [ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md)：

- root-owned 权限文件，无应用层加密；
- Device Token `0600 root:root`；Gateway key/leaf 与含凭据 Caddy 配置 `0640 root:natsume-gateway`；Seat 凭据 `0600 root:root`；
- 全部原子写（temp + fsync + rename），半写不可见；
- 最小版本头，不预建迁移框架；
- identity-before-credentials（`INV-IDENTITY-02`）；
- 损坏不自动重建；窗口重开后的 re-enrollment 只适用于 Server 当前仍为 `enrolled` 的既有 Device。`disabled` / `revoked` Device 没有 Enrollment 恢复捷径；恢复必须走另行评审的显式、受审计 lifecycle/runbook，当前不提供 enable/reactivate API。重新 `SYNC_SECRET` 同样不改变 Device state。

### 5.3 内存

- 使用 secrecy/zeroize 等适合的类型；
- 避免 Clone、Debug、serde 到通用结构；
- 明文尽量限制在一个 use case；
- 不跨 async task/channel 广播；
- panic/error 不包含值；
- 临时文件默认禁止，确需使用必须 owner-only、原子清理。

## 6. 身份启动决策

| 已有 identity-bound 产物 | 当前硬件身份 | 持久化 ID | 结果 |
|---|---|---|---|
| 否 | 有效来源 ≥ 2 且派生成功 | 无 | 允许首次 Enrollment |
| 否 | 来源不足/不可用 | 无 | fail closed，等待修复 |
| 是 | 匹配 | 有 | 继续使用本地凭据 |
| 是 | 不可用 | 有 | fail closed |
| 是 | 不匹配 | 有 | fail closed，按替换/恢复处理 |
| 是 | 匹配但凭据文件损坏 | 有 | fail closed，按恢复处理 |

禁止：

- 删除 identity file 后自动注册；
- 删除凭据文件后自动注册；
- 选择多个候选之一；
- 将 configured-disk copy 当作原机器；
- 创建安装实例 ID 作为硬件身份 fallback。

## 7. Fail-closed 矩阵

| 故障 | 允许继续 | 禁止 |
|---|---|---|
| Server 暂时离线 | 使用已验证本地 LKG | 新签发、新 binding、新 secret |
| Device Token 被吊销/失效，或 resolved Device 非 `enrolled` | 本地有限 BLOCKED 页面 | 控制面连接 |
| Gateway cert 无效 | 旧有效 LKG 或 BLOCKED | 未验证 READY |
| Machine ID 不可用/冲突 | 诊断和人工恢复 | 使用本地凭据、Enrollment |
| 凭据文件损坏 | 诊断、备份、人工恢复 | 自动重建凭据 |
| Command 陈旧 | 报告 stale | 部分应用 |
| Caddy reload 失败 | 保留已验证旧配置或 BLOCKED | 继续代理候选配置 |
| upstream 非 TLS | BLOCKED / 不激活注入 | 明文注入凭据 |
| Home prepare 不确定 | 恢复/清理 | 启动受管 session |
| Agent 不属于当前 session | 拒绝/lease 过期 | 执行会话动作 |
| Observed 上传失败 | 本地结果保留，稍后重报 | 回滚已成功原子动作 |
| audit 写失败 | 整个领域事务回滚 | 无审计提交敏感变更 |

## 8. 恢复原则

1. 先保存证据，再修改状态。
2. 先确认身份和当前 epoch，再执行恢复。
3. 不把删除凭据、identity、journal 或数据库行当作首选修复。
4. provisioning 恢复只处理当前 singleton：已关闭时零写入；已打开时通过 audited CAS close-once。它绝不从 audit、backup 或启动路径推断应当重新打开窗口。
5. 恢复动作使用与正常路径相同的校验和权限边界。
6. 任何身份重建、凭据替换、Device replacement 和 contest reset 都必须人工明确授权（窗口重开本身受审计）。
7. 恢复后用 Observed、Drift、certificate inspection 和 audit 验证，而不是只看服务进程已启动。
8. 无法证明旧状态安全时进入 BLOCKED，而不是尝试“最大可用性”。
9. runbook 中的每个 destructive step 必须有备份/rollback 条件。
10. 重开 provisioning window、重投 Enrollment 或保留 disabled Device 的 token/certificate row 都不得被 runbook 解释为重新启用；只有 `enrolled` Device 可进入 re-enrollment issuance。

具体恢复步骤在对应 Phase 实现后编写；当前不保留未建系统的目标操作流程。

## 9. 审计

`AuditEvent` 是唯一通用的历史/证据表。敏感 mutation 与其 AuditEvent 必须由同一个 guarded operation 在同一 transaction 中原子写入；该 operation 自行插入 audit row 和业务 mutation。fresh `audit_event_id` 可作为 typed operation input，但已持久化的同 ID 或预插入 audit row 不能重放为新 mutation 的凭据。audit 写入、redaction 验证或 commit 失败时，mutation 必须回滚。每个事件的明确字段是：

```text
audit_event_id
occurred_at
actor
action_kind
resource_type
resource_id?                 # nullable
result
reason_code?                 # nullable
correlation_id
group_correlation_id?        # nullable；仅查询/审计分组
redacted_detail_json         # typed、allowlisted、已脱敏；承载 revisions、counts 和其他 event-specific detail
```

以下动作必须审计：

- CSV upload / preview / Import Commit / discard / expiry，以及 candidate/payload 的终态删除；
- import stale reject、expiry reject、binding-stale reject；
- no-op Import Commit（仅 lineage；无 configuration/binding revision bump、无 Target churn；`credential_revision` 仍在每次已提交 import 推进）；
- material Import Commit 的 atomic unbind impact（受影响 Seat/Device 计数与允许的 identity）；
- account/credential revision 变化；
- provisioning 窗口正常开启/关闭与 `system:recovery` close-once；
- Device Enrollment（含凭据替换请求的 operator 批准与拒绝、替换语义 re-enrollment 与“旧连接存活时被替换”异常事件）、retire、delete；
- Device Token 吊销；
- binding/unbind；
- Command create（首次持久化）与同 ID/不同 fingerprint 的 conflict 拒绝、`SYNC_STATE`、`SYNC_SECRET` 与终态；同 ID/同 fingerprint 的 replay **不写**新 audit——它是幂等的读等价结果，为它写审计既违反零副作用 replay 规则，又让重试的 client 得以撑大审计表；
- certificate issuance 与证书状态行变化（含 `revoked` / `retired` 终态保留）；
- Session/Home action；
- operator 登录失败限流触发（每个 limiter window 一条，actor 为非人类 system actor；不记录尝试使用的 login name）；
- operator password 的离线重置（`natsume-server reset-operator-password`，含随之终止的该 operator 全部 session）；
- backup/restore/reset；
- 安全恢复和人工 override。

`redacted_detail_json` 只能保存 typed allowlisted evidence，例如适用 revision、变更分类、计数和稳定 reason；不得成为任意 payload dump。审计与普通 surface 不包含 password 明文、private key、Device Token 值、原始 CSV、ciphertext、CSR/certificate body、完整路径或未脱敏 source chain。失败、discard、expiry 与 rollback 的 audit 不得被解释为 confirmed truth 已变更；它们只证明尝试、拒绝或终止发生过。

## 10. 日志和指标脱敏

允许：

- `DevicePk` 或截断/散列后的稳定诊断 ID；
- Command ID；
- stable ErrorCode；
- revision/epoch；
- certificate serial/fingerprint 的受限表示；
- 静态内部失败判别符（编译期常量字符串，不含用户输入、路径或 source chain）；
- 状态和耗时。

默认禁止：

- password/ciphertext；
- Device Token 值（允许 DB 主键或哈希前缀）；
- private key；
- 原始硬件 serial；
- 完整 Machine Hardware ID；
- 用户 Home 路径；
- CSR/certificate 全文；
- CSV 原始行；
- D-Bus/HTTP payload dump；
- error source chain；
- operator cookie/token；
- request fingerprint 哈希值（`request_fingerprint_sha256`）——日志与 metrics 只允许 `request_fingerprint_version` 与「匹配/不匹配」判定。

## 11. 安全变更评审

**没有可定位证据的安全声明不得用于 Gate PASS。**
