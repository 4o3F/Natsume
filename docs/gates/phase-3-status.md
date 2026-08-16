# Phase 3 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-16
> G3：`OPEN`（实现推进中；证据随包登记）

Phase 3（Identity & Enrollment）启动分解。条目通过需可定位 evidence；partial pass 记为未通过。

## 工作包分解（启动定义，2026-08-16）

| WP | 内容 | 状态 |
|---|---|---|
| WP1 | machine-identity 整机组合配方冻结 + claim 层 2-of-3 + 词表统一（ADR-0032 修订） | `DONE`（`6b40ab8`；26 项决策表/golden 测试，缺失标记字节已钉死） |
| WP2 | Server enrollment 面：provisioning window open/close operator API（契约需新增冻结）、enrollment request 受理（同端口独立路由族）、`create_device` 同事务联合签发（Token + Gateway leaf）、`replace_device_credentials` approve-then-claim、`202` 幂等重投轮询、same-SPKI 自动批准 | `DONE`（WP2a `a288918`；WP2b `2907796` + `abb1a1b`；WP2c review 面与 Enrollment 页 `ab7cae6`） |
| WP3 | Client：privileged raw collectors（DMI/disk 实读）、identity file 原子写、identity-first startup、凭据文件 | `DONE`（`721e4a9`；13 针探针/反向探针 + opus 审查 3 项阻断全部修复，凭据写入器按启动分解随 WP4 实数据落地） |
| WP4 | Client enrollment 流程接线 + 替换语义 | `OPEN` |

- WP2a（provisioning window operator API：open / close / read）已随 `a288918` 落地；WP2b（intake + 联合签发 + approve-then-claim writer）随 `2907796` / `abb1a1b` 落地；WP2c（operator list/approve/reject HTTP 面 + Web Enrollment 页 + 5 条 Playwright 场景）随 `ab7cae6` 落地，WP2 关闭。

## WP3 启动分解（2026-08-16 冻结）

- **WP3a helper 采集与派生**：`hardware_identity::collect()` 按模块文档冻结次序实读（sysfs `/sys/class/dmi/id` 直读为首选 → smbios-lib 补缺/冲突校验 → raw-cpuid 仅真实 PSN leaf → procfs MountInfo + sysfs 上溯 + `/run/udev/data` 唯一根整盘）；`[ReadOutcome; 3]` → slot evaluate → 整机 decide 的纯 pipeline 全部在 helper 进程内执行，normalized 值不跨进程；system D-Bus `org.natsume.Privileged1.CollectHardwareCandidates` 返回 sanitized claim，wire 类型与 introspection XML 扩展整机决策字段（decision kind、`machine_hardware_id`、present slot 数），daemon 由此重建 `MachineIdentityDecision` 供 startup 比对。
- **WP3b identity record**：路径 `/var/lib/natsume/identity/identity.json`（目录已由 tmpfiles CI 断言）；内容为严格 JSON `{fleet_namespace_uuid, machine_hardware_id}`（deny unknown fields、canonical lowercase UUID）；`0600 natsume:natsume`；`temp + fsync + rename` 原子写；读取严格分类 Absent / Corrupt / Valid。
- **WP3c identity-first startup**：daemon 启动序 = site.toml 读 `fleet_namespace_uuid` → identity-bound artifact presence 扫描 → `evaluate_local_identity_preflight` → helper 采集 → `evaluate_startup_identity`；`FirstStart` 原子持久化 identity record，`Matched` 通过，其余（Indeterminate / IdentityUnavailable / ResetRequired / record Corrupt / namespace mismatch）fail closed 为 typed 稳定终态；WP3 不触网，终止于 identity decided。
- **WP3d 凭据文件原语**：共享原子写原语（mode/ownership 参数封闭），identity record 为现行消费者；Token / Seat / Gateway 写入器待 WP4 实数据落地，本包不预建。
- 属主基线：ADR-0032/0034 已以同日修订调和为 service-user 所有（daemon 以 `natsume` 运行、helper 禁持凭据，`root:root` 令消费者不可读，属被迫调和）。
- 测试策略：collectors 为薄 I/O adapter（只产 `ReadOutcome`），policy 由纯 crate fixture 穷举；真机全路径证据仍依赖 G0-IN-005（BLOCKED-INPUT）；D-Bus 面用 peer-to-peer socket 做 round-trip 测试，不依赖 CI system bus。

- WP3 审查非阻断挂账：(a) **udev DB 就绪竞态**——首启若 `/run/udev/data` 未就绪，2 槽 ID 落盘后每次重算 3 槽 ID → 永久 `ResetRequired`，unit 未依赖 udev 就绪，WP4 前需处置（排序加固或 FirstStart 完备性策略）；(b) identity record 首启竞态下 `RENAME_NOREPLACE` 失败返回错误而非 noop（systemd 单例化 + 重启收敛为 `Matched`，可接受；贴合 repeat-safe 惯例可改为重读比对）；(c) `atomic_write` 为 create-only 语义（测试已钉死），WP4 Token 轮换写入器须扩展参数而非复用；(d) artifact 扫描为 lstat 语义不计符号链接，且 SIGKILL 残留的 temp 文件会把 clean first start 翻成 fail-closed；(e) 零化不完整（`FromUtf8Error` buffer、SMBIOS 原始表无零化）——helper unit 已补 `LimitCORE=0` 缓解，值不出进程。

## WP2 启动待冻结面（设计项，非 owner 决策）

- provisioning window open/close 的 operator HTTP 面（启动时待冻结；现由上述 WP2a 按 §3.3 route 与 §3.6.4 audit registry 落地，保留本项作为启动记录）
- enrollment request 的 device 侧 wire（路径、`202` + 轮询语义、与 `approveEnrollment` 的资源关系）
- Gateway leaf 签发的服务端 CA 材料来源与存放（ADR-0033 权威；G0-IN-006：双根自签）

## 已登记待办

- G3 证据登记需含各包 head 的全绿 CI run（待 owner push）。
- fixture 决策表全路径证据依赖 G0-IN-005 硬件 fixture（BLOCKED-INPUT，工具已就绪）。
- WP2b 不写 `enrollment_requests.state = 'conflict'`：冻结文档只定义 different-SPKI live request 的稳定零写入拒绝，尚未定义该 persisted terminal state 的 writer。
- WP2b 的 credential replacement 旧连接 anomaly audit 等 WSS connection facts 落地后实现；本包禁止预建 WSS 或虚构 live-connection evidence。
- WP2b 审查提出的 create 先到先得抢注序列（M1）已处置：ADR-0033 以 2026-08-16 修订补全论证——抢注 Device 无 Binding 且在真机替换批准后凭据即失效，残余风险由「Seat Binding 晚于 Enrollment 队列清空与实物核对」的阶段次序纪律约束，不引入新机制。
- WP2c 审查非阻断挂账：(a) list 单条坏行 fail-closed 为整表 500，设备侧无法投毒，为已接受的健壮性权衡；(b) Path extractor 对非 UTF-8 路径段返回 axum 原生 `text/plain` 400（无 `code`/`correlation_id` body），为既有面（`/devices/{id}/actions/*` 相同），待统一收口；(c) Playwright mock 字面量未以 `satisfies` 钉住 generated schema 且 `e2e/` 不过 tsc，contract drift 由 `enrollment-contract.test.ts` + 页面 generated types 守护，e2e 环空缺；(d) Enrollment 页轮询失败且存在 stale data 时静默展示旧行（401 有全局跳转，5xx 无提示），与 Preparation 页同惯例；(e) `EnrollmentStoreError::from_application` 将语义无关变体折叠为 `SigningFailed`（WP2b 既有，不可达但降低日志辨识度）。
