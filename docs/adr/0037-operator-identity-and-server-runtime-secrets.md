# ADR-0037: Operator identity and Server runtime secret material

> Status: `ACCEPTED`
> Scope: Operator → Server 信任边界的身份持久化，以及 Server 进程运行时秘密材料的来源与所有权
> Consolidates: —
> Supersedes: —
> Superseded by: —

## Context

[架构](../architecture.md) §5 已把 Operator → Server 列为独立信任边界，[契约](../contracts.md) §3.1 已冻结 `admin` / `viewer` 两级固定角色，[安全与恢复](../security-recovery.md) §2 已把 Operator session 列为受管资产。三处都只确认边界存在，没有确定 operator 身份保存在哪里、会话如何表示。

同样未确定的是 Server 自身运行时秘密材料的来源。打包配置已固定数据库与 vault 主密钥的路径，但没有 Server TLS leaf 与私钥的位置；`postinstall` 按 [仓库布局](../repository-layout.md) §8 不生成任何密钥。

Phase 1 要交付 operator auth 与 audit 原子性，必须先确定这两类持久化事实的所有者。

## Decision

**Operator 身份是 Server 持久化事实。** operator 账户与会话保存在 Server 数据库，不保存在配置文件、进程内存或 Web。账户持有固定角色；会话在数据库只保存凭证哈希与绝对过期时间（`expires_at_unix_ms`），不做滑动续期。登出、过期与撤销走同一失效路径。角色是封闭枚举，系统中不存在任何角色变更操作，因此当前不存在可审计的角色变化事件；未来若引入角色编辑能力，必须先有新 ADR，再为其建立对应审计条款。operator 会话生命周期的审计集合由 [契约](../contracts.md) §3.6.4 冻结——§9 的「Session/Home action」是 [领域模型](../domain-model.md) §13 的本地运行时动作，不覆盖 operator 会话。

**Server 运行时秘密材料位于 Server 私有状态目录，不位于包管理的配置目录。** vault 主密钥与 Server TLS 私钥都是 `natsume-server` 用户私有的受限文件，权限不宽于 `0600`，其目录不宽于 `0700`。包管理的配置目录只保存非秘密站点输入与公开信任根。

**vault 主密钥只由显式 `natsume-server bootstrap` 生成。** `bootstrap` 从固定 package config 取得路径；主密钥缺失时以 CSPRNG 生成并用 temp + fsync + rename 原子写入，已存在时只读取并校验，不重写、不轮换、不复制到普通路径。`natsume-server serve` 只读取并校验已经存在的主密钥，缺失时 fail closed，绝不以隐式 first-start detection 创建。

**2026-08-15 修订**：vault record 加密冻结为 XChaCha20-Poly1305（RustCrypto `chacha20poly1305`），密钥为 32 字节主密钥本体，nonce 为每 record 24 字节 CSPRNG；主密钥文件格式不变。record 密文不绑定 AAD——主密钥与数据库同属同一 0700 私有状态目录与同一 uid；此为有意立场（与 ADR-0032 不保存 ciphertext format/AAD/key version 一致），若未来出现跨信任边界的密文搬运再重开。

**2026-08-20 修订**：删除 `import_payload` vault type 与 `record_type`/`subject_id` 列。`accounts` 为父表，无 `credential_vault_record_id`。`server_vault_records` 只保存当前 Account 的 DOMjudge 密码；以 `account_id` 为 PRIMARY KEY 且 `REFERENCES accounts(account_id) ON DELETE CASCADE`，无独立 `vault_record_id`。同一事务先插 Account 再插 vault；删除 Account 级联删除 vault。

**2026-08-20 修订（会话过期列）**：`operator_sessions` 的绝对过期列为 `expires_at_unix_ms`（INTEGER UTC epoch milliseconds）。删除 RFC 3339 TEXT 与 `strftime` CHECK。HTTP 不暴露该列；cookie 仍用 `Max-Age=57600`，不发送 `Expires`。`device_id`、`binding_id` UUID occupancy、vault `account_id` PK、无 `revision_counters`、无 `import_payload` 均保持。

**唯一 first admin 只由离线、交互式 `bootstrap` 创建。** operator 在 TTY 上以 `natsume-server` 用户手工运行该 subcommand；login name 从 TTY 读取，password 不回显地读取两次。account 只在 `operator_accounts` 为空时与 typed audit row 同事务创建；重复 bootstrap 零业务写入并失败。password 不得来自 argv、环境变量、systemd credential、配置文件或 packaging script，`postinstall` 不得调用 `bootstrap`。

**first admin 密码遗失由离线 `reset-operator-password` 恢复。** `bootstrap` 是一次性的：在此之前，唯一 admin 的密码遗失只能走破坏性 single-lifetime reset，代价与故障不相称。因此新增第三个 subcommand，其固定序列由 [契约](../contracts.md) §2.1 冻结：以 `create_if_missing = false` 打开已存在的数据库并运行 migration、从 TTY 读取目标 login name 与两次不回显的新 password、在同一事务内替换该 operator 的 PHC string、删除其全部当前 session row 并写入 `system:password-reset` 的 typed audit row（**2026-08-14 修订**：该 actor 由 `system:recovery` 拆分为专用值，登记于[契约](../contracts.md)审计词汇注册表）。它不创建账户、不生成或轮换 vault 主密钥、不做 TLS preflight、不启动 listener；未知 login name 零写入并以非零状态退出。输入 channel 仍限于交互式 TTY，因此不放宽本 ADR 的 secret-channel 边界。

**Server TLS leaf 与私钥由离线流程提供，Server 只读取。** 缺失、不可读或与证书不匹配时启动失败：不自签、不降级、不生成自签回退。

不可协商边界：除上述 operator password 的交互式 TTY 输入外，秘密材料不经 argv、环境变量、systemd credential、配置、packaging script、日志或 Web 传递；operator 会话凭证明文只存在于响应与浏览器 cookie，Server 只保存哈希。Operator 身份不能获得 Device Token、Gateway certificate 或 WSS 控制面身份（`INV-CERT-01`）。

## Alternatives

- **operator 凭证写入普通配置文件**：零新增表，但把密码哈希放进普通配置与 [安全与恢复](../security-recovery.md) §2「不存在暂时放到普通配置这一例外」冲突，且无法审计角色变更。
- **会话只存进程内存**：少一张表，但重启即失效、不可审计，与 [安全与恢复](../security-recovery.md) §8「恢复后用证据验证，而不是只看服务进程已启动」不一致。
- **vault 主密钥由 runbook 预置**：更严格，但恢复 runbook 按 [安全与恢复](../security-recovery.md) §8 要待对应 Phase 实现后编写，当前会使 Phase 1 开发与 CI 阻塞于尚不存在的流程。
- **Server 自签 TLS leaf 作为开发回退**：会引入「为方便测试禁用证书验证」类路径，[依赖策略](../dependency-policy.md) §4 明确禁止。

## Consequences

### Positive

- operator 身份、角色与会话可审计，且与领域 mutation 共用同一 guarded transaction 与 audit 语义。
- 秘密材料的所有权与权限模型同 Client 侧凭据文件一致，policy scan 与备份边界可统一表达。
- `bootstrap` 把不可重复的 first-admin 与 vault 初始化放在明确的离线 operator action 中；`serve` 的启动路径保持非交互且不会偷偷创建持久化身份或密钥。
- 唯一 admin 密码遗失不再等同于必须执行破坏性 single-lifetime reset；恢复路径与 `bootstrap` 共用同一 TTY-only secret channel、同一 Argon2id profile 与同一 audit 语义，且顺带终止该 operator 的全部现存 session。

### Negative / trade-offs

- schema 相对 Phase 0 基线新增 operator 与会话两类当前事实表，schema 契约测试须同步扩展。
- operator 密码哈希需要具备工作因子的密码哈希算法，属新增依赖，须按 [依赖策略](../dependency-policy.md) §2 单独准入。
- 初次部署增加一个必须由 operator 以 package user 在 TTY 上完成的手工步骤，不能由 unattended install 或 service start 代办。
- `reset-operator-password` 是一条能替换任意 operator 密码的高权限本地路径，其防护完全依赖 Server 主机的 OS 访问控制；这与 [ADR-0030](0030-foundation-deployment-and-delivery-baseline.md) T2「本地 root 不在防护范围」一致，但意味着 Server 主机的物理与账户管理纪律不可放松。
- vault 主密钥的存在性由显式 bootstrap 而非 serve 保证；密钥丢失等同 vault 数据不可恢复，备份边界由恢复 runbook 承担。

## Acceptance basis and revisit trigger

接受依据：[安全与恢复](../security-recovery.md) §2 与 §5 已要求每个秘密有明确 owner、存储与销毁路径，[架构](../architecture.md) §5 已确立 Operator 边界；本决策只把既有原则落到具体所有者，不放宽任何 `INV-*`。

重开条件：出现多操作员并发或外部身份源需求；确有无人值守 first-admin provisioning 需求且能先冻结等强度的 secret channel 与审计边界；vault 主密钥需要轮换或托管到外部 secret store；Server TLS 材料改由自动化签发流程管理。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Contracts](../contracts.md)
- [Security and recovery](../security-recovery.md)
- [Dependency policy](../dependency-policy.md)
