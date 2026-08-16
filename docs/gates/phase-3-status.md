# Phase 3 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-16
> G3：`OPEN`（WP1–WP4 全部 `DONE`；16 主题条目表见下——14 项 `PASS`、1 项 owner 裁定降级、1 项例外移交 Phase 4；关门待补齐断言 head 的全绿 run 登记）

Phase 3（Identity & Enrollment）启动分解。条目通过需可定位 evidence；partial pass 记为未通过。

## 工作包分解（启动定义，2026-08-16）

| WP | 内容 | 状态 |
|---|---|---|
| WP1 | machine-identity 整机组合配方冻结 + claim 层 2-of-3 + 词表统一（ADR-0032 修订） | `DONE`（`6b40ab8`；26 项决策表/golden 测试，缺失标记字节已钉死） |
| WP2 | Server enrollment 面：provisioning window open/close operator API（契约需新增冻结）、enrollment request 受理（同端口独立路由族）、`create_device` 同事务联合签发（Token + Gateway leaf）、`replace_device_credentials` approve-then-claim、`202` 幂等重投轮询、same-SPKI 自动批准 | `DONE`（WP2a `a288918`；WP2b `2907796` + `abb1a1b`；WP2c review 面与 Enrollment 页 `ab7cae6`） |
| WP3 | Client：privileged raw collectors（DMI/disk 实读）、identity file 原子写、identity-first startup、凭据文件 | `DONE`（`721e4a9`；13 针探针/反向探针 + opus 审查 3 项阻断全部修复，凭据写入器按启动分解随 WP4 实数据落地） |
| WP4 | Client enrollment 流程接线 + 替换语义 | `DONE`（`93cd6e2`；5 条真 TLS 端到端场景 + 单一拒绝源静态探针，opus 审查零阻断、3 条非阻断随包修复） |

- WP2a（provisioning window operator API：open / close / read）已随 `a288918` 落地；WP2b（intake + 联合签发 + approve-then-claim writer）随 `2907796` / `abb1a1b` 落地；WP2c（operator list/approve/reject HTTP 面 + Web Enrollment 页 + 5 条 Playwright 场景）随 `ab7cae6` 落地，WP2 关闭。

## WP3 启动分解（2026-08-16 冻结）

- **WP3a helper 采集与派生**：`hardware_identity::collect()` 按模块文档冻结次序实读（sysfs `/sys/class/dmi/id` 直读为首选 → smbios-lib 补缺/冲突校验 → raw-cpuid 仅真实 PSN leaf → procfs MountInfo + sysfs 上溯 + `/run/udev/data` 唯一根整盘）；`[ReadOutcome; 3]` → slot evaluate → 整机 decide 的纯 pipeline 全部在 helper 进程内执行，normalized 值不跨进程；system D-Bus `org.natsume.Privileged1.CollectHardwareCandidates` 返回 sanitized claim，wire 类型与 introspection XML 扩展整机决策字段（decision kind、`machine_hardware_id`、present slot 数），daemon 由此重建 `MachineIdentityDecision` 供 startup 比对。
- **WP3b identity record**：路径 `/var/lib/natsume/identity/identity.json`（目录已由 tmpfiles CI 断言）；内容为严格 JSON `{fleet_namespace_uuid, machine_hardware_id}`（deny unknown fields、canonical lowercase UUID）；`0600 natsume:natsume`；`temp + fsync + rename` 原子写；读取严格分类 Absent / Corrupt / Valid。
- **WP3c identity-first startup**：daemon 启动序 = site.toml 读 `fleet_namespace_uuid` → identity-bound artifact presence 扫描 → `evaluate_local_identity_preflight` → helper 采集 → `evaluate_startup_identity`；`FirstStart` 原子持久化 identity record，`Matched` 通过，其余（Indeterminate / IdentityUnavailable / ResetRequired / record Corrupt / namespace mismatch）fail closed 为 typed 稳定终态；WP3 不触网，终止于 identity decided。
- **WP3d 凭据文件原语**：共享原子写原语（mode/ownership 参数封闭），identity record 为现行消费者；Token / Seat / Gateway 写入器待 WP4 实数据落地，本包不预建。
- 属主基线：ADR-0032/0034 已以同日修订调和为 service-user 所有（daemon 以 `natsume` 运行、helper 禁持凭据，`root:root` 令消费者不可读，属被迫调和）。
- 测试策略：collectors 为薄 I/O adapter（只产 `ReadOutcome`），policy 由纯 crate fixture 穷举；真机全路径证据仍依赖 G0-IN-005（BLOCKED-INPUT）；D-Bus 面用 peer-to-peer socket 做 round-trip 测试，不依赖 CI system bus。

- WP3 审查非阻断挂账：(a) **udev DB 就绪竞态**——已处置：daemon unit 增加 `systemd-udev-settle.service` 排序（gating 调用方即 gating 按需激活的 helper），封住首启 2 槽落盘窗口；settle 已弃用但目标镜像（Ubuntu 24.04）仍提供，超时（默认 120s）后的残余窗口记录在案；(b) identity record 首启竞态下 `RENAME_NOREPLACE` 失败返回错误而非 noop（systemd 单例化 + 重启收敛为 `Matched`，可接受；贴合 repeat-safe 惯例可改为重读比对）；(c) `atomic_write` 为 create-only 语义（测试已钉死），WP4 Token 轮换写入器须扩展参数而非复用；(d) artifact 扫描为 lstat 语义不计符号链接（该语义已随 `1a70127` 统一到 token presence 检查）；SIGKILL 残留的 temp 文件翻转 clean first start——已处置（`1a70127`：`atomic_write` 钉死 `.natsume-tmp` 前缀 + 扫描跳过；改动前默认前缀 `.tmp` 的历史孤儿不受覆盖，预发布无既有部署故不追溯）；(e) 零化不完整（`FromUtf8Error` buffer、SMBIOS 原始表无零化）——helper unit 已补 `LimitCORE=0` 缓解，值不出进程。

## WP4 启动分解（2026-08-16 冻结）

- **WP4a key 与 CSR**：daemon 生成 ECDSA P-256 keypair（rcgen），私钥 PKCS#8 DER 以 create-only 原子写持久化到 `/var/lib/natsume/keys/gateway-key.pk8`（`0640 natsume:natsume-gateway`）；key 先于首次 POST 落盘，响应丢失后重试保持同 SPKI（自愈路径的前提）；已存在 key 一律复用不再生成。CSR 最小化（空 DN、无 SAN——服务端只验 possession）。
- **WP4b HTTP client**：reqwest + rustls，信任根仅 `/etc/natsume/trust/control-ca.crt`，端点取 `/etc/natsume/config.toml` 的 `[server] ip/port`，IP URL 走 rustls IP-SAN 校验，禁 TOFU/危险 verifier；wire 按 OpenAPI `createEnrollmentRequest`（CSR base64 标准填充、SPKI 小写 hex、`protocol_version = 1`、`client_version` 取 crate 版本）。
- **WP4c 轮询语义**：`ENROLLMENT_POLL_INTERVAL_SECONDS = 5` 固定间隔幂等重投；`202` 继续轮询；`ENROLLMENT_REQUEST_REJECTED` 终局——记录后驻留等待现场人员（不重启锤击）；窗口关闭 409 与连接失败/5xx 视为等待态继续轮询；其余 4xx 为自身缺陷 fail closed 非零退出。
- **WP4d finalization**：收到 `201` 后先全部校验再落盘——leaf SPKI 与本地私钥匹配、chain 恰一张且逐字节等于 packaged `local-origin-ca.crt` 的 DER、leaf 经 webpki 以 site `gateway_hostname` 验证通过；随后按 leaf → chain → token 次序原子持久化（`gateway-leaf.der`、`gateway-chain.der` 为 `0640 natsume:natsume-gateway`，`device-token` 为 `0600 natsume:natsume`）；token 存在即 enrolled 标记，任何校验失败零落盘。`atomic_write` 增加封闭的 create-only/replace 写策略参数（replace 供 claim 重签发换发凭据）。
- **状态接线**：identity FirstStart/Matched 后，token 缺失 → enrollment 流程（EnrollmentPending）；token 存在 → Enrolled 驻留；token 存在但 key/leaf 损坏 → fail closed（ADR-0032：不得自动 re-enroll）；token 缺失时的重投属同一 enrollment 的重试（same-SPKI 自愈），非 re-enroll。
- **测试策略**：integration-tests 以真实 server（test PKI、localhost TLS listener）端到端驱动 daemon enrollment 库函数——create 同步签发、replace approve-then-claim、rejected 终局、closed-window 轮询等待、finalization 校验失败零落盘；daemon 单测覆盖文件层与状态判定。WP4 终点为凭据落盘 + Enrolled 驻留，WSS 接线属后续 Phase。

- WP4 审查非阻断挂账（3 条已随包修复：body/decode 传输错误归等待态、私钥 fixture 置换为公开 CSR 钉子、keys 目录 setgid 归组使 0640 组读落地；(a)-(d)/(f)-(i) 已随 Phase-4 前置 sweep `1a70127` 处置）：(a) **时钟偏移**——已处置（`1a70127` 服务端 notBefore 回拨 3600s，`GATEWAY_NOT_BEFORE_BACKDATE_SECONDS`）。残余登记：RTC 大幅回退（如电池耗尽落回出厂日期）仍 fail-closed；unit 加 `time-sync.target` 排序在无 NTP 的离线赛场不产生同步保证故不采用；Phase 4 `ServerHello.server_time_unix_ms` 可作大偏移诊断面，随 Phase 4 WP5 评估；(b) webpki 失败分辨——已处置（`1a70127`：过期/未生效 → `LeafValidityWindow`、名称不匹配 → `InvalidHostname`、其余 → `InvalidChain`，各拒绝位点有钉子测试）；(c) `enroll_until_parked` 循环无测试触达——已处置（`1a70127` 真 TLS replacement 收敛场景，实测跨越一次 5s 轮询）；(d) Replace-over-existing 与「201 丢失后同 SPKI 自愈」客户端侧场景——已处置（`1a70127` 两条端到端场景；chain 文件内容按设计恒等于 packaged origin CA DER，无字节差异可断言）；(e) 旧 token rotation 后 inode 残留 + `response.bytes()` 的 token 明文不零化（与 WP3 零化挂账同类，`LimitCORE=0` 缓解）——保持挂账；(f) token presence lstat 一致性——已处置（`1a70127`，symlink fail-closed）；(g) 测试 accessor 门面——已处置（`1a70127` `fixture` cargo feature + ci-rust 默认 features 构建守卫）；(h) token 尾字节钉死——已处置（`1a70127`，与 OpenAPI pattern 对齐，16 个合法尾字符经穷举验证）；(i) `script(1)` 缺失诊断——已处置（`1a70127`）。

## G3 条目（16 主题，引自 roadmap §Phase 3）

| # | 主题 | 状态 | 代表证据（具名测试/断言） |
|---|---|---|---|
| 1 | identity 决策表全路径 | `PASS` | `machine-identity::claim_decision_table_covers_all_343_status_combinations` + 15 项决策表/golden + daemon startup 13 项 + helper 12 项 |
| 2 | configured-disk copy fixture | `DEFERRED-NONBLOCKING` | owner 2026-08-16 裁定随 G0-IN-005 降级：实地采集归首次 provisioning（ADR-0032）；决策函数侧由 `machine_identity_startup.rs::copied_configured_state_on_different_hardware_uses_standard_reset_path` 覆盖 |
| 3 | 窗口开/关负向 | `PASS` | `db/provisioning/tests.rs::close_open_window_failures_persist_no_partial_effect` 等 |
| 4 | 正常 open/close audit+CAS | `PASS` | `operator_open_close_open_cycle_advances_revision_and_writes_exact_audits` |
| 5 | open-window restart/restore close-once 与 closed-window 零写入 | `PASS` | `schema_contract_tests.rs::startup_recovery_closes_an_open_window_exactly_once`；closed-window 零写入见 enrollment intake 测试 |
| 6 | 联合签发事务原子性 | `PASS` | `db/enrollment/tests.rs::csr_spki_mismatch_and_duplicate_issuance_audit_leave_zero_partial_state` |
| 7 | CSR SAN ignore | `PASS` | `closed_window_is_zero_write_and_create_issuance_is_secret_safe_and_site_authoritative`（hostile SAN/CN/serial 全弃置） |
| 8 | create 同步签发与 replacement 审批分支 | `PASS` | `handler/enrollment/tests.rs::operator_approve_claim_and_reject_poll_flows_are_end_to_end` |
| 9 | `202` 幂等重投轮询返回同一 live request | `PASS` | `pending_replay_conflict_and_rejected_poll_have_exact_device_http_semantics` + `client_enrollment.rs` 真 TLS 场景 |
| 10 | approval 零签发与 claim 时窗口复检 | `PASS` | 同上端到端流（approve 后零签发、claim 时复检窗口） |
| 11 | 窗口关闭时未 claim 请求转 `expired` | `PASS` | `expire_enrollment_requests` 写入器测试（operator close 与 recovery close-once 两路径） |
| 12 | same-SPKI 自动批准重试 | `PASS` | `db/enrollment/tests.rs::same_spki_retry_reissues_once` + 客户端侧 `lost_issue_response_self_heals_with_same_spki_over_real_tls` |
| 13 | 同 hardware ID 不同 SPKI 稳定拒绝 | `PASS` | `rejected_hardware_blocks_same_and_rotated_spki_until_window_close` |
| 14 | operator 拒绝的稳定码 | `PASS` | `ENROLLMENT_REQUEST_REJECTED` 端到端（服务端 + 客户端 typed terminal step） |
| 15 | 替换语义与旧连接异常审计 | `PASS`（替换语义）+ **例外：旧连接 anomaly audit 移交 Phase 4 WP3** | `revoked_and_disabled_devices_require_approval_then_reactivate_on_claim` + `replacement_over_existing_artifacts_converges_via_enroll_until_parked`；anomaly audit 需 WSS connection facts，移交记录见下方待办与 [`phase-4-status.md`](phase-4-status.md) |
| 16 | package upgrade 保留 identity/凭据 | `PASS`（断言 2026-08-16 补齐） | `hosted-lifecycle.sh` client 半：seed identity/key/token → 重装 → 字节与 mode/owner 不变断言；已知限制：跨版本 upgrade 无已发布前版（与 G0 条目 12 同源） |

## 已登记证据（G3）

- 主题 1、3–14 与 15 前半：[ci run 31932403934](https://github.com/4o3F/Natsume/actions/runs/31932403934)（head `a544b96`，2026-08-16，全 lane 绿，含 WP1–WP4 全部提交 `6b40ab8`/`a288918`/`2907796`/`abb1a1b`/`ab7cae6`/`721e4a9`/`93cd6e2`）。
- 主题 12 客户端侧与 15 的 enroll_until_parked 场景（`1a70127`）、主题 16 断言（2026-08-16 补齐）：锚定其 head 的全绿 run 后关门。

## 关闭条件

16 主题全部 `PASS` 或有 owner 裁定的降级/移交记录，且证据锚定全绿 CI run；主题 15 的 anomaly-audit 例外随 Phase 4 WP3 回访。

## WP2 启动待冻结面（设计项，非 owner 决策）

- provisioning window open/close 的 operator HTTP 面（启动时待冻结；现由上述 WP2a 按 §3.3 route 与 §3.6.4 audit registry 落地，保留本项作为启动记录）
- enrollment request 的 device 侧 wire（路径、`202` + 轮询语义、与 `approveEnrollment` 的资源关系）
- Gateway leaf 签发的服务端 CA 材料来源与存放（ADR-0033 权威；G0-IN-006：双根自签）

## 已登记待办

- G3 证据登记需含各包 head 的全绿 CI run（待 owner push）。
- fixture 决策表全路径证据依赖 G0-IN-005 硬件 fixture（BLOCKED-INPUT，工具已就绪）。
- WP2b 不写 `enrollment_requests.state = 'conflict'`：冻结文档只定义 different-SPKI live request 的稳定零写入拒绝，尚未定义该 persisted terminal state 的 writer。
- WP2b 的 credential replacement 旧连接 anomaly audit 等 WSS connection facts 落地后实现；本包禁止预建 WSS 或虚构 live-connection evidence。（已由 Phase 4 WP3 吸收，见 [`gates/phase-4-status.md`](phase-4-status.md)。）
- WP2b 审查提出的 create 先到先得抢注序列（M1）已处置：ADR-0033 以 2026-08-16 修订补全论证——抢注 Device 无 Binding 且在真机替换批准后凭据即失效，残余风险由「Seat Binding 晚于 Enrollment 队列清空与实物核对」的阶段次序纪律约束，不引入新机制。
- WP2c 审查非阻断挂账：(a) list 单条坏行 fail-closed 为整表 500，设备侧无法投毒，为已接受的健壮性权衡；(b) Path extractor 对非 UTF-8 路径段返回 axum 原生 `text/plain` 400（无 `code`/`correlation_id` body），为既有面（`/devices/{id}/actions/*` 相同），待统一收口；(c) Playwright mock 字面量未以 `satisfies` 钉住 generated schema 且 `e2e/` 不过 tsc，contract drift 由 `enrollment-contract.test.ts` + 页面 generated types 守护，e2e 环空缺；(d) Enrollment 页轮询失败且存在 stale data 时静默展示旧行（401 有全局跳转，5xx 无提示），与 Preparation 页同惯例；(e) `EnrollmentStoreError::from_application` 将语义无关变体折叠为 `SigningFailed`（WP2b 既有，不可达但降低日志辨识度）。
