# Natsume V2 安全与恢复不变量

> 状态：`NORMATIVE`  
> 适用范围：所有进程、协议、持久化、打包、运维和恢复  
> 规则：放宽任何 `INV-*` 必须先有已接受 ADR  
> 校准基线：[ADR-0022](adr/0022-deployment-facts-and-trust-assumptions.md) 的部署事实与信任假设

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

不承诺防护（ADR-0022 T2）：

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
| DOMjudge password | Server vault；Client `0600` 凭据文件；渲染的 Caddy `/login` 注入配置（`0640`） | secret sync、Daemon 渲染、Caddy `/login` header 注入 | API、Observed、日志、Agent、状态页 |
| Fleet namespace UUID | 配置/Server truth | identity derivation | 无秘密属性 |
| Machine Hardware ID | identity file/Server metadata | identity、Enrollment | 不作为密码或 token |
| Operator session | Server/Web secure session | operator API | Device control |

秘密必须有明确 owner、生命周期、存储、使用者和销毁路径。不存在"暂时放到普通配置"这一例外。含凭据的 Caddy 配置是 secret artifact，与凭据文件同级管理（[ADR-0024](adr/0024-domjudge-autologin-via-xheaders.md)）。

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

允许的秘密路径必须使用专用类型、最短生命周期和 redacted 结果。Import 普通边界只使用 opaque `preview_token` 与 redacted 分类/计数/identity/revision 证据。

按 [ADR-0028](adr/0028-single-operator-import-and-secret-evidence-scope.md)：password-derived digest/length/fingerprint 不再作为独立禁止类别维护（审计仅面向内部管理员，F9）；工程默认仍不输出这些值。F9 失效时该条必须重审。

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

Machine Hardware ID 是站点 namespace 下的稳定标识（派生配方见 [ADR-0025](adr/0025-deterministic-hardware-identity-recipe.md)），不是 secret、token 或 certificate。网络认证必须使用 Device Token、TLS 和 operator auth。

原始硬件 serial 只在最小采集边界短暂出现；日志、Server API 和 fixture 只保存规范化/匿名化结果。

**验证入口：** library tests、helper boundary、fixture review、log scan。

### `INV-IDENTITY-02`：身份检查先于凭据使用

Device 启动必须先验证当前 Machine Hardware ID，再读取或使用任何 identity-bound 产物（Device Token、Gateway key、Seat 凭据、LKG）。

当 identity-bound 产物已存在且当前身份不可获得、与持久化值不匹配或有效来源不足（少于 2 个，见 ADR-0025）时，Device 必须 fail closed。凭据文件损坏同样 fail closed，不能自动重建或自动 re-enroll。

**验证入口：** startup decision table、fault tests、configured-disk-copy fixture、recovery runbook。

### `INV-CERT-01`：签发面在窗口内封闭

签发路径固定为两段：

```text
server-auth TLS（全部入口，预置 trust + IP-SAN 验证）
  → provisioning 窗口内 Enrollment：签发 { Device Token + Gateway certificate }
  → WSS 控制面（token 认证）；SYNC_STATE / SYNC_SECRET 不签发任何东西
```

窗口关闭时不存在任何签发路径；窗口状态默认关闭、变更受审计、故障恢复后不自动开启（[ADR-0021](adr/0021-provisioning-window-certificate-issuance.md)）。Enrollment 之外、operator API 与任何 Command 都不能获得 token 或证书。

**验证入口：** 窗口关闭负向测试、OpenAPI/DB/schema tests、无 token upgrade 拒绝测试。

### `INV-CERT-02`：授权属性由 Server 派生

Gateway SAN、hostname、profile、EKU 和 validity 必须由 Server 站点配置与冻结 policy 派生。CSR 自报字段不授予权限，只证明 possession 与公钥结构。

同一 `MachineHardwareId` 窗口内重复 Enrollment 为受审计的替换：旧 token 失效、新产物签发。

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
- 绑定当前 Device、Seat、assignment revision 和 credential revision；
- 使用 durable Command；
- 在 Device 写入前再次校验；
- 返回 redacted 结果；
- 可审计。

**验证入口：** authorization tests、stale revision tests、secret scan、audit tests。

### `INV-COMMAND-01`：Command durable 且幂等

Server 先持久化 Command 再投递；Device 先持久化 receipt/journal 再确认。相同 `command_id` 不得重复副作用。

相同 ID 不同 payload 必须 conflict；崩溃和重连后必须能恢复既有状态。

**验证入口：** crash/fault injection、duplicate delivery、journal durability、reconnect tests。

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

DOMjudge 凭据只通过 Caddy 对 `/login` 路由的 header 注入进入数据面（[ADR-0024](adr/0024-domjudge-autologin-via-xheaders.md)）；**Caddy → DOMjudge upstream 必须为 TLS**，至少覆盖 `/login`。upstream 非 TLS 时不得激活注入配置。本机 loopback HTTPS 不替代该要求。

**验证入口：** 非 TLS upstream 拒绝激活测试、`/login` 之外路由无注入头的负向测试、含凭据配置的权限/日志脱敏检查。

### `INV-SESSION-01`：Session/Home 使用 epoch 且不拥有 Caddy

所有 Session/Home 动作绑定当前 epoch。陈旧 Agent、陈旧 UI action、陈旧 Home cleanup 必须拒绝。

Home 无法证明安全时不得启动受管 session。Session lock/unlock/terminate 不调用 Caddy、不改变 Caddy config/状态。遮罩类 UI（如未来实现）是呈现层，不是完整性边界；完整性依靠 `SESSION_TERMINATE` 与数据面 BLOCKED（[ADR-0027](adr/0027-single-image-desktop-cycle.md)）。

**验证入口：** 桌面 capability 清单、epoch race tests、Caddy call counter、Home fault recovery。

## 4. PKI 结构

两条平行的单层链，各自只被真正需要验证它的一方信任：

| 链 | 签发 | 信任方与分发 |
|---|---|---|
| control CA → Server TLS leaf | 离线生成，runbook 保管；Server leaf 经批准的离线流程签发 | Device 与操作员浏览器；经 package/debconf 预置 `control-ca.crt` |
| origin CA → 各 Device Gateway leaf | origin CA key 在 Server 上，provisioning 窗口内经 Enrollment 签发 | 各设备本机浏览器；origin CA 证书经包构建期注入 `local-origin-ca.crt` |

不存在 Device Identity CA（[ADR-0023](adr/0023-wss-control-channel-with-device-token.md)）。每张 Gateway certificate 跟踪：serial、SPKI fingerprint、not-after、status。不建吊销分发机制；revoked/retired 仅作台账。

## 5. 秘密存储

### 5.1 Server vault

- 应用层 AEAD；
- ciphertext、nonce、format version 和 associated data 明确；
- DB 备份不应单独恢复出明文；
- key 不通过 argv、env、日志或 Web；
- secret read 只通过专用 use case；
- key rotation/format migration 原子且可恢复；
- audit 记录访问动作但不记录值。

CSV / candidate import 的 password-bearing 材料只进入 encrypted staging 与 secret-safe persistence。staging 失败、过期清理、discard 与未提交 candidate 不得把明文残留到普通 surface；未成功 Import Commit 不得改变 confirmed contest configuration。

### 5.2 Client 凭据文件

按 [ADR-0026](adr/0026-client-secrets-as-permission-files.md)：

- root-owned 权限文件，无应用层加密；
- Device Token `0600 root:root`；Gateway key/leaf 与含凭据 Caddy 配置 `0640 root:natsume-gateway`；Seat 凭据 `0600 root:root`；
- 全部原子写（temp + fsync + rename），半写不可见；
- 最小版本头，不预建迁移框架；
- identity-before-credentials（`INV-IDENTITY-02`）；
- 损坏不自动重建；恢复 = 窗口重开 re-enrollment / 重新 `SYNC_SECRET`。

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
| Device Token 被吊销/失效 | 本地有限 BLOCKED 页面 | 控制面连接 |
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
4. 恢复动作使用与正常路径相同的校验和权限边界。
5. 任何身份重建、凭据替换、Device replacement 和 contest reset 都必须人工明确授权（窗口重开本身受审计）。
6. 恢复后用 Observed、Drift、certificate inspection 和 audit 验证，而不是只看服务进程已启动。
7. 无法证明旧状态安全时进入 BLOCKED，而不是尝试"最大可用性"。
8. runbook 中的每个 destructive step 必须有备份/rollback 条件。

具体恢复步骤在对应 Phase 实现后编写；当前不保留未建系统的目标操作流程。

## 9. 审计

以下动作必须审计：

- CSV upload / preview / Import Commit / discard；
- import stale reject、expiry reject、binding-stale reject；
- no-op Import Commit（仅 lineage；无 revision bump、无 Target churn）；
- material Import Commit 的 atomic unbind impact（受影响 Seat/Device 计数与 identity）；
- account/credential revision 变化；
- provisioning 窗口开启/关闭；
- Device Enrollment（含替换语义 re-enrollment 与"旧连接存活时被替换"异常事件）、retire、delete；
- Device Token 吊销；
- binding/unbind；
- `SYNC_STATE`、`SYNC_SECRET`；
- certificate issuance 与台账状态变化；
- Session/Home action；
- 角色变化；
- backup/restore/reset；
- 安全恢复和人工 override。

审计事件最少包含 actor、action、resource、result、time、correlation、revision 和 redacted change。审计与普通 surface 不包含 password 明文、private key 或 Device Token 值。失败、discard、expiry 与 rollback 的审计记录不得被解释为 confirmed truth 已变更。

## 10. 日志和指标脱敏

允许：

- `DevicePk` 或截断/散列后的稳定诊断 ID；
- Command ID；
- stable ErrorCode；
- revision/epoch；
- certificate serial/fingerprint 的受限表示；
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
- operator cookie/token。

## 11. 安全变更评审

安全变更的自检清单见 [`CONTRIBUTING.md`](../CONTRIBUTING.md)。

**没有 evidence locator 的安全声明不得用于 Gate PASS。**
