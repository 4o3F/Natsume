# Natsume V2 安全与恢复不变量

> 状态：`NORMATIVE`  
> 适用范围：所有进程、协议、持久化、打包、运维和恢复  
> 规则：放宽任何 `INV-*` 必须先有已接受 ADR

本文件定义少量稳定不变量。字段级负向案例属于 contract tests、policy scan、probe 和 runbook，不在架构正文中重复枚举。

## 1. 威胁模型

Natsume 主要防护：

- 未认证或错误 Device 获取控制面权限；
- Enrollment 被滥用为通用证书签发；
- 网络输入扩展成任意本地特权；
- 密码通过 API、日志、UI、事件或错误泄漏；
- Machine ID 改变、复制或不可用时误用旧 vault；
- 陈旧 Command/revision/epoch 操作当前状态；
- 未验证证书或配置进入 Caddy READY；
- Session Agent 获得过多权限；
- 恢复操作通过删除证据“修好”故障；
- 文档或 Gate 在无证据时宣称安全已验证。

不承诺防护：

- 本地 root；
- 物理访问和固件篡改；
- 已被完全控制的 Server；
- DOMjudge 自身漏洞；
- 竞赛网络之外的通用终端安全；
- Server HA；
- Session lock 作为网络隔离。

## 2. 信任与秘密清单

| 资产 | 保存位置 | 可使用者 | 禁止消费者 |
|---|---|---|---|
| Server control private key | Server 受限文件/approved secret store | Server TLS adapter | Web、Device、Helper |
| Offline control root key | 离线介质 | PKI ceremony | 运行中 Server |
| Device private key | Client vault/受限 key store | Enrollment/QUIC adapter | Agent、Helper、Server |
| Gateway private key | Client vault/受限 key store | Gateway/Caddy adapter | Agent、Helper、Server |
| DOMjudge password | Server vault、按需 Client vault | secret sync、受限数据面 adapter | API、Observed、日志、Agent |
| Client vault root key | root-owned local material | Device Daemon | Agent、Helper、Caddy |
| Fleet namespace UUID | 配置/Server truth | identity derivation | 无秘密属性 |
| Machine Hardware ID | identity file/Server metadata | identity、Enrollment | 不作为密码或 token |
| Operator session | Server/Web secure session | operator API | Device control |

秘密必须有明确 owner、生命周期、存储、使用者、轮换和销毁路径。不存在“暂时放到普通配置”这一例外。

## 3. 核心不变量

### `INV-SECRET-01`：秘密不可观察

密码和 private key **不得**进入：

- 通用 HTTP response；
- Web 持久化状态；
- Target；
- Observed；
- SSE/ChangeEvent；
- Audit diff；
- 普通导出；
- 日志和指标；
- Session Agent；
- Helper；
- Caddy 状态页；
- ErrorCode detail 或 source chain。

对 password-bearing CSV / candidate import，下列材料同样 **不得**进入上述 ordinary surfaces（以及 outbox / metric）：

- password 值；
- password length；
- password fingerprint；
- raw CSV content hash；
- 其他由 password-bearing CSV 内容派生的 digest；
- Server 内部 candidate digest/revision（仅允许存在于 encrypted staging / secret-safe persistence）。

允许的秘密路径必须使用专用类型、最短生命周期、应用层加密和 redacted 结果。Import 普通边界（含 API、Browser 可见响应、audit、log、metric、SSE、outbox）只可使用 opaque `preview_token` 与 redacted 分类/计数/identity/revision 证据；不得要求 Browser 持有或回传 password-derived digest/fingerprint/length 或内部 candidate digest。

**验证入口：** secret scan、API/schema snapshot、日志测试、UI/e2e、Command redaction、import redaction、package scan。

### `INV-INPUT-01`：网络输入只能映射到封闭契约

网络输入 **不得**成为任意：

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

Machine Hardware ID 是站点 namespace 下的稳定标识，不是 secret、token 或 certificate。网络认证必须使用证书和 operator auth。

原始硬件 serial 只在最小采集边界短暂出现；日志、Server API 和 fixture 只保存规范化/匿名化结果。

**验证入口：** library tests、helper boundary、fixture review、log scan。

### `INV-IDENTITY-02`：身份检查先于 vault

Device 启动必须先验证当前 Machine Hardware ID，再打开任何 identity-bound vault/artifact。

当 identity-bound artifact 已存在且当前身份：

- 不可获得；
- 不唯一；
- 与持久化值不匹配；
- 质量不足；

Device 必须 fail closed。vault 解密失败也必须 fail closed，不能自动创建新 Device 或新 vault。

**验证入口：** startup decision table、fault tests、configured-disk-copy probe、recovery runbook。

### `INV-CERT-01`：证书阶梯不可跨越

证书路径固定为：

```text
server-auth Enrollment
→ Device Identity certificate
→ mandatory-mTLS QUIC
→ active SYNC_STATE
→ Gateway certificate
```

Enrollment 只签 Device Identity certificate。Gateway certificate 不能从 Enrollment、匿名连接、operator 通用 API 或通用证书接口获得。

**验证入口：** OpenAPI/DB/schema tests、anonymous QUIC test、Gateway subprotocol integration test。

### `INV-CERT-02`：授权属性由 Server 派生

Gateway SAN、hostname、profile、EKU 和 validity 必须由 Server 当前 Target/policy 派生。CSR 自报字段不授予权限。

request 必须绑定 authenticated Device、active Command、generation、assignment revision 和 SPKI。相同 request/SPKI 幂等；不同 SPKI 冲突。

**验证入口：** CSR SAN ignore test、stale/conflict tests、certificate inspection。

### `INV-STATE-01`：非秘密状态显式应用

Target 只含非秘密数据且本身惰性。CSV、binding 或配置变化不得自动产生远端副作用。

只有人工发起的 `SYNC_STATE` 可以应用 Target。Observed 是实际状态来源，Drift 是纯比较。

Import 不创建 Operation/Command，不自动 `SYNC_STATE` 或 `SYNC_SECRET`，也不表示 Device 已同步。失败、discard、过期、stale baseline、binding/preview mismatch 与 transaction rollback **均不得**改变 confirmed configuration、binding、Target truth 或相关 revision。material/no-op import 的完整 revision 与 outbox 规则以 [领域模型](domain-model.md) 为准。

**验证入口：** domain tests、outbox tests、UI authorization、offline Device tests、import non-mutation tests。

### `INV-SECRET-02`：秘密只由人类明确同步

没有自动 `SYNC_SECRET`。Import Commit、binding 变更或 confirmed contest configuration 替换本身 **不得** 创建或暗示 secret sync；密码材料只进入 encrypted staging / Server vault / 明确发起的 secret-sync 路径，且不得以 length、fingerprint 或 password-derived digest 出现在 ordinary surface。

每次 secret sync 必须：

- 由授权操作员明确发起；
- 绑定当前 Device、Seat、assignment revision 和 credential revision；
- 使用 durable Command；
- 在 Device 写入前再次校验；
- 返回 redacted 结果；
- 可审计。

**验证入口：** authorization tests、stale revision tests、secret scan、audit tests、import non-auto-sync tests。

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

- 无 vault/PKI/Caddy 所有权；
- 不访问 Server；
- 只处理 typed local snapshot/action；
- 只由 XDG Autostart 直接启动。

**验证入口：** D-Bus policy、package scan、network namespace/seccomp/AppArmor policy（如采用）、negative method tests。

### `INV-DATAPLANE-01`：数据面必须 fail closed

Caddy 只有在证书、私钥、SAN/profile、配置、generation 和本地健康检查全部验证后才能进入 READY。

否则保持已验证 LKG，或进入 BLOCKED/503。状态页只显示有限 allowlist 数据，绝不代理未验证 upstream。

**验证入口：** config validation、bad cert/key/SAN tests、load rollback、status page security tests。

### `INV-SESSION-01`：Session/Home 使用 epoch 且不拥有 Caddy

所有 Session/Home 动作绑定当前 epoch。陈旧 Agent、陈旧 UI action、陈旧 Home cleanup 必须拒绝。

Home 无法证明安全时不得启动受管 session。Session lock/unlock/terminate 不调用 Caddy Admin，也不改变 Caddy config/hash/generation/status。

**验证入口：** desktop probe、epoch race tests、Caddy call counter、Home fault recovery。

## 4. PKI 结构

### 4.1 控制面

- 离线 Control Root：不在运行中 Server；
- Server control leaf/chain：用于 operator HTTPS、Enrollment server-auth 和/或按配置分离的 Server endpoint；
- Device Identity CA/issuer：只签 Device Identity profile；
- Device Identity leaf：用于 Device mTLS client auth。

具体 root/intermediate 拆分可以按部署调整，但 profile、key usage 和签发路径必须隔离，不能用一个“万能证书”。

### 4.2 本地数据面

- Local Origin Root/Intermediate 或等价受控签发链；
- Gateway certificate：Server 在 authenticated `SYNC_STATE` 内签发；
- Gateway private key：只在 Client 本地生成和保存；
- Managed Browser trust：通过部署时受控安装，不使用 TOFU。

### 4.3 证书状态

每类证书独立记录：

- subject/identity；
- serial；
- SPKI fingerprint；
- profile；
- not-before/not-after；
- chain fingerprint；
- issuance policy revision；
- revoked/retired 状态；
- last validation result。

不得用一个 `certificate_ready` 覆盖 Device 和 Gateway。

## 5. Vault

### 5.1 Server vault

- 应用层 AEAD；
- ciphertext、nonce、format version 和 associated data 明确；
- DB 备份不应单独恢复出明文；
- key 不通过 argv、env、日志或 Web；
- secret read 只通过专用 use case；
- key rotation/format migration 原子且可恢复；
- audit 记录访问动作但不记录值。

CSV / candidate import 的 password-bearing 材料与内部 candidate digest 只进入 encrypted staging 与 secret-safe persistence。staging 失败、过期清理、discard 与未提交 candidate **不得**把明文或 password-derived ordinary-surface 证据残留到 API、audit、log、metric、SSE 或 outbox；未成功 Import Commit 不得改变 confirmed contest configuration。

### 5.2 Client vault

- 随机 32-byte root key；
- Machine Hardware ID 作为 HKDF salt/绑定输入之一，而非 key；
- root-owned 权限；
- identity-before-vault；
- Device/Gateway key、certificate、credential 和 LKG 按用途分隔；
- format version；
- 原子写、fsync/rename 策略由 adapter 证明；
- decrypt failure 不自动重置。

### 5.3 内存

- 使用 secrecy/zeroize 等适合的类型；
- 避免 Clone、Debug、serde 到通用结构；
- 明文尽量限制在一个 use case；
- 不跨 async task/channel 广播；
- panic/error 不包含值；
- 临时文件默认禁止，确需使用必须加密、owner-only、原子清理。

## 6. 身份启动决策

| 已有 identity-bound artifact | 当前硬件身份 | 持久化 ID | 结果 |
|---|---|---|---|
| 否 | 唯一且质量合格 | 无 | 允许首次创建 |
| 否 | 不可用/冲突 | 无 | fail closed，等待修复 |
| 是 | 匹配 | 有 | 继续打开 vault |
| 是 | 不可用 | 有 | fail closed |
| 是 | 不匹配 | 有 | fail closed，按替换/恢复处理 |
| 是 | 匹配但 vault 解密失败 | 有 | fail closed，按 vault recovery 处理 |

禁止：

- 删除 identity file 后自动注册；
- 删除 vault 后自动注册；
- 选择多个候选之一；
- 将 configured-disk copy 当作原机器；
- 创建安装实例 ID 作为硬件身份 fallback。

## 7. Fail-closed 矩阵

| 故障 | 允许继续 | 禁止 |
|---|---|---|
| Server 暂时离线 | 使用已验证本地 LKG | 新签证书、新 binding、新 secret |
| Device cert 过期/撤销 | 本地有限 BLOCKED 页面 | control mTLS |
| Gateway cert 无效 | 旧有效 LKG 或 BLOCKED | 未验证 READY |
| Machine ID 不可用/冲突 | 诊断和人工恢复 | 打开 vault、Enrollment |
| vault 解密失败 | 诊断、备份、人工恢复 | 自动新建 vault |
| Command 陈旧 | 报告 stale | 部分应用 |
| Caddy load 失败 | 保留已验证旧配置或 BLOCKED | 继续代理候选配置 |
| Home prepare 不确定 | 恢复/清理 | 启动受管 session |
| Agent 不属于当前 session | 拒绝/lease 过期 | 执行会话动作 |
| Observed 上传失败 | 本地结果保留，稍后重报 | 回滚已成功原子动作 |
| audit/outbox 写失败 | 整个领域事务回滚 | 无审计提交敏感变更 |

## 8. 恢复原则

1. 先保存证据，再修改状态。
2. 先确认身份和当前 epoch，再执行恢复。
3. 不把删除 vault、identity、journal 或数据库行当作首选修复。
4. 恢复动作使用与正常路径相同的校验和权限边界。
5. 任何身份重建、证书替换、Device replacement 和 contest reset 都必须人工明确授权。
6. 恢复后用 Observed、Drift、certificate inspection 和 audit 验证，而不是只看服务进程已启动。
7. 无法证明旧状态安全时进入 BLOCKED，而不是尝试“最大可用性”。
8. runbook 中的每个 destructive step 必须有备份/rollback 条件。

具体恢复步骤将在对应 Phase 实现后写入 `runbooks/`；当前不保留未建系统的目标操作流程。

## 9. 审计

以下动作必须审计：

- CSV upload / preview / Import Commit / discard；
- import stale reject、expiry reject、preview token mismatch reject、binding freshness mismatch reject；
- no-op Import Commit（仅 lineage/redacted AuditEvent；无 confirmed-content 突变、无 contest/credential/assignment revision bump、无内容变化 outbox/Target churn）；
- material Import Commit 的 atomic unbind impact（仅当 preview 已授权的待解绑 bound Seat 集合非空；受影响 Seat/Device 计数与 identity，动作如 `UNBIND_ON_COMMIT`）；
- account/credential revision 变化；
- Device Enrollment、retire、delete、replacement；
- binding/unbind；
- `SYNC_STATE`、`SYNC_SECRET`；
- certificate issuance/revocation；
- Session/Home action；
- role/permission 变化；
- backup/restore/reset；
- Gate/平台状态签收；
- 安全恢复和人工 override。

审计事件最少包含 actor、action、resource、result、time、correlation、revision 和 redacted change。Import 相关 redacted change **只允许**：

- redacted diff classification 与各类计数；
- `is_noop` / commit 结果类别；
- 受影响 Seat code 与允许展示的 Device identity；
- before/after `ContestConfigurationRevision`、`AssignmentRevision`、`CredentialRevision`（仅实际变化时）；
- import lineage 标识（如 `import_id`、correlation）；
- binding impact 摘要（计数与 `UNBIND_ON_COMMIT` 等动作标签）。

Import 审计与 ordinary surface **不得**包含 password 值、password length、password fingerprint、raw CSV、raw CSV hash、password-derived digest，或 Server 内部 candidate digest。只有指定安全审计角色可查看敏感元数据，但仍不包含秘密值。

失败、discard、expiry、stale baseline、binding freshness mismatch、preview token mismatch 与 transaction failure 的审计记录不得被解释为 confirmed truth 已变更。

## 10. 日志和指标脱敏

允许：

- `DevicePk` 或截断/散列后的稳定诊断 ID；
- Command/Operation ID；
- stable ErrorCode；
- generation/revision；
- certificate serial/fingerprint 的受限表示；
- 状态和耗时。

默认禁止：

- password/ciphertext；
- password length、password fingerprint 或 password-derived digest；
- raw CSV hash / Server 内部 candidate digest；
- private key；
- 原始硬件 serial；
- 完整 Machine Hardware ID；
- 用户 Home 路径；
- CSR/certificate 全文；
- CSV 原始行；
- D-Bus/HTTP payload dump；
- error source chain；
- operator cookie/token；
- opaque 之外的 preview 绑定材料明文（不得把内部 candidate digest 当诊断字段打印）。

## 11. 安全变更评审

每个安全相关变更至少回答：

- 资产和攻击者是谁？
- 新增了哪个 trust boundary？
- 哪个进程获得了新 capability？
- 是否能用更窄的 typed contract？
- secret 在哪里出现、保存多久、如何清零？
- identity/revision/epoch 如何绑定？
- 失败时保留什么、拒绝什么？
- 重试是否幂等？
- audit 是否原子？
- 正向、负向、故障注入和恢复证据在哪里？
- 是否需要更新 `INV-*`、ADR、Gate 和 runbook？

没有 evidence locator 的安全声明不得用于 Gate PASS。
