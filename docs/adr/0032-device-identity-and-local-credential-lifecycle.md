# ADR-0032: Device identity and local credential lifecycle

> Status: `ACCEPTED`
> Scope: Machine Hardware ID derivation, Device lifecycle, identity-first startup, Client credential artifacts, and Server vault
> Consolidates: ADR-0002, ADR-0006, ADR-0010, ADR-0011, ADR-0025, ADR-0026
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

异构工作站曾发生 MAC 地址冲突。Machine Hardware ID 必须确定地表示物理机器生命周期、让已 provision 的磁盘复制可被发现，并能在纯测试中验证，而不把 identity policy 放进 privileged process。

Device 在证明当前硬件身份前不得读取或使用旧机器绑定的凭据。Client 与 Server 的 at-rest threat boundary 不同：当前 Client 只防非 root 选手，Server 还必须防独立数据库文件或备份泄露。

## Decision

### 固定身份配方

**2026-08-15 修订：** 以下 R1–R7 冻结身份配方的实现细节，既有概括条款均按本修订解释。

- **R1（anchor literal 与顺序）：** 三个 slot 依次为 `dmi_system_uuid`、`dmi_board_serial`、`first_disk_serial`，顺序固定；`first_disk_serial` 的语义是「支撑根文件系统的整盘」。
- **R2（normalization）：** 先去除首尾 ASCII whitespace，再删除分隔字符 `-`、`_`、`:` 与内部空格，随后转为小写；`dmi_system_uuid` slot 还必须解析为 UUID 并规范化，不能解析的值归为 `malformed`。
- **R3（placeholder）：** placeholder 一律在 normalization 后比较；对初始冻结表的匹配键是 normalized value 的「仅保留字母数字字符」projection，但 R2-normalized value 本身不变，R7 derivation 仍原样使用它；拒绝空值、全 `0`、全 `f`，以及初始冻结表 `{tobefilledbyoem, defaultstring, systemserialnumber, notspecified, none, unknown, na, invalid, 0123456789}`，新发现的厂商 placeholder 只能通过后续带日期的 ADR 修订追加。该 projection 覆盖 `To Be Filled By O.E.M.`、`N/A` 等 R2 有意保留 `.` 或 `/` 的真实厂商写法；若 strong slot 漏过这类 placeholder，会在全 fleet 派生同一个共享 candidate，风险与 v1 事故同构。
- **R4（quality）：** quality 是 slot 固有且恒定的等级：`DmiSystemUuid=strong`、`DmiBoardSerial=strong`、`FirstDiskSerial=medium`；磁盘会被克隆或迁移（v1 事故即为克隆磁盘），两个主板锚定的 strong source 使 2-of-3 判定保持可靠。
- **R5（status）：** 有效值映射为 `present`；ENOENT 类映射为 `unavailable`；EACCES 类映射为 `permission_denied`；解析或编码失败映射为 `malformed`；placeholder 映射为 `rejected_placeholder`；平台不存在该 attribute 时映射为 `unsupported`；`conflict` 只属于 claim 层，采集不得产生该状态。
- **R6（completeness）：** 任一 slot 为 `unsupported` 时整体为 `unsupported`；否则任一 slot 非 `present` 时为 `temporarily_unavailable`；否则为 `complete`。
- **R7（candidate derivation）：** 在公开且不可变的 Fleet namespace UUID 下派生 UUIDv5，name bytes 严格为 anchor literal、单个 NUL byte（`0x00`）与 normalization 后的 value bytes；该 domain separation 保证不同 slot 的相同原始值产生不同 candidate，且仅 `present` slot 产生 candidate。

**Layering 边界：** R7 只冻结 per-slot candidate derivation；原条款中的 whole-machine ID combination recipe 与 2-of-3 decision 继续由 claim 层拥有，其精确 byte recipe 在 Phase 3 实现时冻结。现有 combined `derive_candidate` / `anchor_priority` vocabulary 属于 claim 层；它与新 slot label 的差异是已知 vocabulary split，将在 Phase 3 wiring 时统一。

**2026-08-16 修订**：claim 层整机配方冻结——按 ANCHOR_ORDER 逐槽拼接 `anchor_literal ++ 0x00 ++（present 槽的 R2 归一化值 | 单字节 0x01 缺失标记）++ 0x00`，在同一 Fleet namespace 下派生 UUIDv5 得 `machine_hardware_id`；判定为 2-of-3——任一槽 `unsupported` 即整体 `unsupported`，否则 `present` 槽数 ≥2 才允许派生，非 present 槽以缺失标记参与拼接；slot/claim 词表已统一。

**2026-08-24 修订（aggregate evidence quality）**：仅对已经通过上述 2-of-3 判定的 claim，将 present slots 的固有 quality 从高到低排序并取第二个值，作为 Enrollment wire 的 aggregate `EvidenceQuality`。它表达形成 quorum 的最低质量：`strong + strong`（无论第三个 medium 是否 present）为 `strong`，`strong + medium` 为 `medium`。该值在首次 Enrollment attempt 前随 transaction material crash-safe 持久化；exact replay 读取持久化值，不因后续启动时 slot 可用性变化而重算。它由 candidate key 签名，但 Server 不接收 raw slots，不能独立复算；Panel 必须标为 Device self-reported advisory evidence。它不改变 derivation bytes、2-of-3 eligibility、unsupported fail-closed 或 startup strict equality，也不构成 authenticator。

开发期匿名 fixture collector 位于 `client/privileged-helper/examples/collect_identity_fixture.rs`，只输出上述匿名化 slot 结果与 completeness，不输出原始硬件值。

- Machine Hardware ID 是 lifecycle identifier，不是 authenticator；网络认证使用 Server-authenticated TLS 与 Device Token。
- 唯一来源是 DMI system UUID、DMI motherboard serial、第一块 system disk serial；MAC 地址排除。
- 统一大小写、空白与分隔符，拒绝 placeholder、全零、全 `F` 等无效值；至少需要两个有效来源。
- 按固定 source slot 顺序组合有效/缺失值，并在公开、不可变的 Fleet namespace UUID 下派生 UUIDv5。
- startup 重算结果必须与 persisted ID 完全一致；不得选择“最接近”候选或猜测设备仍是同一台。
- `machine-identity` crate 只拥有 normalization、placeholder filtering、2-of-3 decision 与 derivation；raw hardware collection 留在 privileged adapter，raw serial 不进入 Server surface 或普通日志。

**2026-08-16 修订（派生管线执行位置）**：整机纯管线（slot evaluate → 2-of-3 decide）在 privileged helper **进程内**执行，helper 经 D-Bus 只返回 sanitized claim（per-slot 匿名 candidate、决策类别与派生出的 `machine_hardware_id`），normalized 硬件值因此不跨进程。这不与「privileged Helper 直接返回最终 ID」的被拒方案冲突：被拒的是把 policy 的**所有权**放进 root boundary；policy 与其纯测试面仍完整归 `machine-identity` crate 所有，helper 仅作为调用方。Daemon 由 sanitized claim 重建决策用于 startup 比对。

### Identity-first startup 与 Device lifecycle

- Device Daemon 的第一个应用流程是 identity decision；所有 identity-bound adapter 和 credential read 都必须位于其后，不新增独立 Identity Guard service。
- 无绑定 artifact 时，只有有效 derivation 才允许首次 Enrollment；已有 artifact 时，证据不足、ID mismatch 或凭据损坏均 fail closed。
- Daemon 不得通过删除 identity/credential artifact 自动获得 fresh registration，也不得自动 re-enroll。
- `machine_hardware_id` 在一个 Device lifecycle 内不可编辑；Server `devices.device_id` 是独立 TEXT primary key（canonical lowercase hyphenated UUIDv7），不由硬件数据派生，也不 merge、split 或复用。`revoked` 永久终止该旧 identity；同一物理机再次接入只能由现场 operator 执行显式 destructive reprovision，完整清除旧 identity-bound material 并生成全新 control/Gateway key、CSR 与 `enrollment_id`，再经人工审核创建新的 `device_id`。这不是 daemon 对“文件缺失”的自动恢复；旧 Device row/key/Resume 保持终态。

**2026-08-20 修订**：原 `devices.device_pk` 已原位更名为 `device_id`，同一 UUIDv7 surrogate。
- 硬件替换或 revoked Device 的 destructive reprovision 都结束旧 lifecycle，需要显式重开 provisioning window、以全新 material 重新 Enrollment、人工重建 binding，再由显式 Command 同步。

### Client artifact 与 Server vault

- Device Token、Seat credential 与 identity record 文件为 `0600 natsume:natsume`；Gateway private material 和含凭据 Caddy 配置为 `0640 natsume:natsume-gateway`（`natsume-caddy` 经组读取）。
- Client artifact 必须由其 service user 所有，使用 `temp + fsync + rename` 原子写入；secret 不通过 env、argv、普通配置或 system credential delivery。

**2026-08-16 修订（artifact 属主与打包基线调和）**：早期 `root` 属主词表与打包基线不可共存——Device Daemon 以专用 service user `natsume` 运行（packaging sysusers/service，CI 断言），Privileged Helper 被架构禁止持有或读取 Token/私钥，因此 `0600 root:root` 会令唯一合法消费者自身不可读。当前 service-user 属主不改变 confidentiality boundary：威胁主体是非 root 的 contest user，其对上述文件零可读；原子写入与 zeroization 条款不变。

**2026-08-19 ADR-0038 foundation 修订**：当前 Token artifact 继续有效至 atomic flag day。Daemon 已在 identity-first gate 与持久化 ID 复核后，于 `/var/lib/natsume/control` create-only 建立 `control-key-1.pk8` 与 closed dormant manifest；control key 不复用 Gateway key，也不参与当前 Enrollment/WSS authority。任何已有 control artifact 都进入 identity-bound preflight scan；key/manifest 丢失、损坏或不匹配必须 fail closed，不能自动替换或把 Machine Hardware ID 当作恢复 credential。Pending/active authority manifest transition仍属后续cutover。
- Client 不增加 application-level encryption；其 confidentiality boundary 是 ownership、mode 与当前非 root attacker scope。
- secret 使用短生命周期、zeroization-aware 类型，不进入通用 `Debug`、serde、日志、指标、audit 或 error chain。
- Server vault 继续使用 application-level AEAD；`accounts` 为父表。`server_vault_records` current row 只有 `account_id`（PRIMARY KEY REFERENCES accounts ON DELETE CASCADE）、`nonce` 与 `ciphertext`，不保存 `vault_record_id`、`record_type`/`subject_id`、ciphertext format/AAD/key version、timestamp、rotation 或 migration metadata。同一事务先插 Account 再插 vault。Client 文件选择不降低 Server 备份泄露边界。

## Alternatives

- `/etc/machine-id`、installation ID、单一硬件来源或 MAC：无法同时处理 copied disk、placeholder 和既有冲突。
- privileged Helper 直接返回最终 ID：把 policy 放入 root boundary，削弱纯测试。
- 独立 Identity Guard service：增加 ordering、state sharing 与 recovery race。
- 可编辑 ID、自动 merge 或复用 `device_id`：破坏证书、binding 与 audit lifecycle。
- Client AEAD vault、通用 external secret manager 或 hardware trust root：当前 threat model 与 fleet baseline 不支持其复杂度。
- Server plaintext、permission-only storage 或用 Machine Hardware ID 作为 vault key：不能满足数据库备份泄露边界。

## Consequences

### Positive

- 身份配方固定、可审查，并可用 anonymized fixture 穷举测试。
- copied/changed disk 不能静默使用旧凭据；privileged collection 与 policy ownership 分离。
- Client recovery 收敛为权限、原子性和显式 operator action，Server at-rest protection 保持独立。

### Negative / trade-offs

- 选定硬件部件更换可能改变身份并要求 re-enrollment。
- placeholder 规则和 2-of-3 可行性必须持续由真实 fixture 证明。
- Client artifact 对 local root/physical attacker 可读；这些攻击者明确在当前 scope 外。
- Server vault 的应用层 AEAD 与备份恢复边界仍需要受限运行与 recovery 成本。

## Acceptance basis and revisit trigger

证据必须覆盖 placeholder、missing source、2-of-3、all-invalid、copied disk、strict startup ordering、permission denial、atomic-write interruption，以及 Server vault wrong-key、tamper 与 recovery。

当目标平台提供稳定的标准硬件身份、真实 fleet 无法满足固定 2-of-3、出现正式维修迁移语义、多进程确需共享 pre-start gate，或 threat model 纳入 local root/physical attacker 时重开。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Security and recovery](../security-recovery.md)
- [Contracts](../contracts.md)
- [Repository layout](../repository-layout.md)
- [Dependency policy](../dependency-policy.md)
- [Supported platform](../supported-platform.md)
