# Natsume V2 依赖与供应链策略

> 状态：`NORMATIVE`  
> 目标：依赖可审计、边界明确、锁文件可重现，不用通用库掩盖架构问题

## 1. 总则

1. Cargo 独占 Rust dependency graph 和 `Cargo.lock`。
2. pnpm 独占 Web dependency graph 和 `pnpm-lock.yaml`。
3. `just` 只分发命令。
4. nFPM 只映射已构建 artifact。
5. 所有生产依赖必须有真实 consumer、明确用途和 owner。
6. 版本、source、feature 和 checksum 必须可审计。
7. 不以“未来可能用到”为由提前加入依赖。
8. 运行时、postinstall 和 first boot 不从公网下载组件。

## 2. Rust 依赖准入

新增 dependency 的 PR 必须说明：

- 使用模块；
- 解决的问题；
- 为何标准库/现有依赖不够；
- enabled features；
- transitive/runtime closure；
- 安全和许可证影响；
- 替代方案；
- 删除条件。

默认：

- `default-features = false`，只开启需要 feature；
- 禁止 git dependency 用于发布基线，除非有临时 ADR 和固定 commit；
- 禁止未固定的 path 外部依赖；
- 同一库版本由 workspace 统一；
- unsafe-heavy 或 native dependency 需要额外评审；
- crypto/TLS/serialization/database 依赖需要 owner；
- production dependency 必须进入 locked CI 和 advisory/license scan。

通用编码、协议语法、文档语法和 CLI parsing 优先使用已维护且 feature closure 可审计的社区库；第一方代码只保留 Natsume 特有的 closed-world validation、稳定错误映射和 fail-closed 策略，不复制通用 parser/serializer。

CLI argument parser 只允许分派封闭 runtime mode，不得演变为配置、路径或 secret transport；缺失、未知 mode 与额外参数必须在任何文件系统访问前失败。

## 3. 错误处理

第一方 Rust domain/application 使用 SNAFU typed error。

规则：

- domain error 保留业务语义；
- source chain 只进入受限内部日志；
- adapter 穷举映射到稳定 ErrorCode；
- 不通过 Display 文本做业务判断；
- library 不 `panic!`、`unwrap`、`expect` 处理可恢复输入；
- secret/path 不进入 error context；
- `anyhow`/`thiserror` 不作为第一方统一错误模型，除非 ADR 明确局部例外；
- ErrorCode crate 不依赖 Axum、Prost、zbus 或具体持久化实现。

## 4. 加密与秘密

允许的选择必须来自经过维护的 Rust crypto ecosystem，并使用公开审计的构造。当前基线包括 AEAD、HKDF、SHA-2、secrecy/zeroize 等类型。operator password hashing 必须使用 memory-hard algorithm 与显式 work factor；`hkdf` 与 `sha2` 都不得作为 password KDF。`sha2` 仍可用于对高熵 session credential 做单向 hash。

禁止：

- 自制 cipher/MAC/KDF；
- 复用 nonce；
- 把 Machine ID 当作 key；
- 在 serde/debug 中暴露 secret；
- 通过 env、argv、unit text 或普通配置传 secret；
- 在 `bootstrap` provisioning 或 `reset-operator-password` 恢复时，通过交互式 TTY 之外的任何 channel 输入 operator password；两条路径上 TTY 都是唯一允许的输入 channel，password 不得来自 argv、环境变量、systemd credential、配置或 packaging script；
- 对 Argon2 password salt 调用 `SaltString::generate` 或引入第二套 RNG stack；salt 必须由 workspace 唯一的 OS CSPRNG（`getrandom`）生成并以 `SaltString::encode_b64` 编码；
- 依赖外部 shell 工具完成核心加密；
- 为方便测试禁用证书验证。

依赖升级必须运行已知向量、wrong-key、tamper 和 crash recovery 测试。

## 5. TLS 与控制通道

当前全部入口为 server-auth TLS；production Token/Enrollment/WSS 仍按 `DEPRECATED` 但有效至 flag day 的 [ADR-0033](adr/0033-enrollment-and-device-control-boundary.md) 运行，[ADR-0038](adr/0038-unified-ordinary-wss-device-control-authority.md) 只定义尚未生效的 ordinary-WSS Ed25519 目标。Operator HTTP、Enrollment 与 Device WSS 共用同一 rustls 栈与同一 TCP 端口，不引入第二个协议栈。

必须显式：

- trust roots；
- server name/IP-SAN；
- ALPN；
- protocol version；
- 0-RTT/early data 保持关闭；
- WS subprotocol version；
- max frame/connection limits；
- 签发侧的 certificate profile 与 key usage（Gateway）。

测试 helper 不能被 production build 导出为 dangerous verifier。当前 Device Token 比对必须常数时间，直到 atomic flag day 删除该 surface。

### 5.1 Batch 0 Ed25519 feasibility 准入

- **consumer**：仅 `integration-tests/tests/ordinary_wss_ed25519_feasibility.rs` 的 private listener；Batch 0 的 Server、daemon、`device-protocol` 与 packaging artifact 均不是 consumer。
- **问题**：证明普通 server-auth TLS 1.3 + RFC 6455 101 后可以用 PKCS#8 Ed25519 key 完成 connection-local Challenge/Proof、strict verify、exact ClientInit hash binding 与 clean close。
- **dependency/features**：workspace 准入 `ed25519-dalek 2.2`，关闭 defaults，只启用 `alloc`、`pkcs8`、`zeroize`；key/nonce entropy 继续使用 workspace `getrandom 0.4`，不启用第二套 RNG stack。
- **closure/licenses**：dev-only closure包含 `curve25519-dalek 4`、`ed25519 2`、`signature 2`、`pkcs8/der/spki/const-oid`、`fiat-crypto` 与 `sha2 0.10`；许可证为既有allowlist内的 BSD-3-Clause / MIT / Apache-2.0 组合。该closure不得因workspace feature unification进入production package，后续production准入需单独复审。
- **供应链/重复版本**：`deny.toml` 显式启用 `licenses.include-dev` 与 `bans.multiple-versions-include-dev`。除 workspace `cargo deny check` 外，Batch 0 必须运行 `cargo deny --manifest-path integration-tests/Cargo.toml check`，使 dev-only Dalek 闭包进入 advisory/license/source 与 duplicate-version 检查；`sha2 0.10.9` 只允许一个带移除条件的 exact-version skip，不得因该 closure 新增通用放宽。若 production 采用，必须重新登记实际 reverse-dependency 图与移除条件。
- **边界**：test-private fixed frame不是 production protocol、generated code 或 future wire substitute；listener使用private CA和正常rustls验证，不导出dangerous verifier、feature或环境开关。
- **替代方案**：手写Ed25519、shell/OpenSSL probe、复用Gateway key均不接受。
- **迁移条件**：后续production batch若采用该依赖，必须把consumer/owner迁入明确production crate并重新审查feature/closure；若目标否决，则连同isolated test和dependency删除。

- **2026-08-17 WebSocket 依赖记录**：workspace 对 `tokio-tungstenite 0.29.0` 使用精确 pin，并有意启用其私有 `__rustls-tls` feature，以避开平台 TLS 与公共根证书等不需要的 feature；该私有 feature 变化时必须显式复审后再升级。axum 的服务端 WSS 与 daemon 的客户端 WSS 会把 tungstenite 的 `rand 0.9` / `getrandom 0.3` 以及 RFC 6455 握手所需的 `sha1` 同时链接进**生产 server 与生产 daemon**，因此两边都受 locked CI、`cargo deny` 与 feature closure 审查约束。
- **2026-08-17 生产 packaging graph**：入包 Rust binary 必须从隔离 target directory 中的显式生产 package 集合构建，因为 workspace-wide build 会把 integration-only feature 统一进生产 artifact；package smoke 必须通过 compiler `cfg` 断言拒绝此类泄漏。

## 6. SQL 和持久化

- Diesel 与 `diesel_migrations` 仅由数据库 adapter 使用，application、domain 和 transport 不直接依赖；
- 业务 CRUD 优先使用 Diesel Query DSL；SQLite PRAGMA、schema introspection 和无法清晰表达的专有查询可使用参数绑定的 raw SQL；
- 同步 Diesel 操作通过共享 r2d2 pool 执行；每个完整数据库 use case 只进入一次 blocking task，事务始终占用同一连接；
- domain 不依赖 SQL row type；
- migration 从空库和升级路径测试；
- 不引入第二个 ORM、SQLite driver 或数据库层；
- SQLite WAL 是初始部署选择，不在 domain API 中暴露；
- backup/restore 通过 runbook 和 integration test 验证。

## 7. Web 依赖

Web 只通过 pnpm workspace 管理。Dependency lifecycle script 默认不得执行；`pnpm-workspace.yaml` 的 `allowBuilds` 必须对每个有 build script 的 package 显式记录 `true` 或 `false`，analytics/telemetry postinstall 保持禁用。

新增 dependency 必须说明：

- browser/runtime 或 dev-only；
- bundle 影响；
- 许可证和供应链；
- 是否已有相同能力；
- 无障碍和安全影响；
- SSR/Node runtime 是否意外引入。

禁止：

- Web 自行维护 API schema；
- 在 production bundle 中引入只为构建使用的 generator；
- 将 password 放入 state persistence/analytics；
- 运行时从 CDN 拉取 UI/runtime；
- 使用 package script 隐式改写 generated artifacts 而不 clean diff。

## 8. Slint / Session Agent

成熟 GUI 属于 Phase 6，但 contract 已冻结：

- build-time compiled Slint；
- Slint `winit` backend；
- Skia renderer；
- 一个 XDG Autostart 直接启动的 resident process；
- 初始 hidden；
- typed snapshot 到来后 lazy create/show；
- UI event loop 在正确线程；
- 无 systemd user unit；
- 无 bootstrap/run handoff；
- 无环境 descriptor；
- 无 runtime interpreter。

禁止把以下作为产品 GUI 实现：

- 直接拼装 `winit`、`softbuffer`、`tiny-skia`、`cosmic-text`；
- Qt/GTK/WebKit/Electron；
- zenity/kdialog/yad/xdg-open；
- Node/Python/JVM GUI helper；
- tray/background app framework；
- display-manager 私有 API；
- 下载式字体/runtime。

最终 Slint feature set、version 和 runtime closure 必须在当期冻结镜像与单一桌面环境的 capability 清单上实测后冻结；每次镜像 bump 重新验证，不维持永久双桌面矩阵。

### 8.1 Session Agent Slint 1.15 准入记录

- 使用模块：`client/session-agent`。`slint` 是生产依赖，`slint-build` 仅在构建期把受审查的 `ui/session_agent.slint` 编译为 Rust；开发用 `ui_probe` example 不进入 nFPM 映射。
- 解决的问题：为 resident-hidden Session Agent 提供 typed `SessionUiSnapshot` 到 lazy window 的最小实测路径，并为目标 VM 的 CJK、HiDPI、X11 映射与 Skia 渲染测量提供构建期固定的 probe。标准库和现有依赖不提供窗口、文字布局或渲染能力。
- enabled features：关闭 default features，仅启用 `compat-1-2`、`std`、`backend-winit-x11`、`renderer-skia`；锁文件解析为 Slint `1.15.1`。不启用 runtime interpreter、Qt backend、Wayland backend 或其他 renderer。
- transitive/runtime closure：运行时进入 Slint core、winit X11 backend、Skia renderer、X11/GL 装载、Fontconfig/Freetype，以及 `image`（jpeg/png 解码）、`resvg`/`usvg`/`tiny-skia` SVG 栈、`rustybuzz`/`ttf-parser` 文字整形与 `softbuffer`；仅 avif/exr/`rav1e` 等编码器与编译器解析工具停留在构建宿主侧（`slint-build`、`i-slint-compiler`）。二进制的直接 ELF NEEDED 集冻结于 `packaging/client/session-agent.needed` 并由 package smoke 断言，对应 Deb 依赖已在 nFPM manifest 声明。Skia 静态 archive 链入 Session Agent，系统动态库闭包须以当期目标 VM 的 `ldd` 和 package lifecycle 结果为准；本记录不把开发机结果提升为目标证据。运行时、安装期与 first boot 均不得下载 UI 或 renderer 组件。
- 安全和许可证影响：选择 Slint 的 `GPL-3.0-only` 分支，与本项目 `AGPL-3.0-or-later` 发布条件兼容；新增的 `BSD-2-Clause` 来自 Slint 图片/SVG closure。GUI、字体与 native renderer 扩大 unsafe/native 攻击面，继续受 locked build、`cargo deny`、package smoke 和目标 VM probe 约束。generated-UI module 是唯一的 `allow(unsafe_code)` 位置，crate root 保持 `deny(unsafe_code)`；该例外只因 Slint 生成的 `ItemTreeVTable` 代码需要 unsafe，边界等同于上游依赖内部的 unsafe。Slint 1.15 closure 当前还精确忽略三个“停止维护”公告：`paste` 的 `RUSTSEC-2024-0436`、`ttf-parser` 的 `RUSTSEC-2026-0192`、`rustybuzz` 的 `RUSTSEC-2026-0206`；它们不是通用 advisory 放宽，升级 closure 消除对应 crate 时必须删除。
- 替代方案：按 ADR-0035 已拒绝直接拼装低层 GUI 栈、Qt/GTK/Web runtime、外部 GUI helper、runtime `.slint` 解释和第二套 launcher；这些方案不重新开放。
- 删除条件：Session Agent 不再承担本地 typed presentation，或新的 ADR 明确替换 GUI 边界时，连同 build script、Slint workspace dependency、probe 和对应 deny 例外一起删除；仅完成 probe 不能作为保留闲置依赖的理由。
- 例外登记：本节的 `allow(unsafe_code)` 生成代码边界与三条 RUSTSEC 停止维护忽略的 owner 均为仓库所有者，登记日 2026-08-14，复查触发为每次 Slint closure 版本变更；过期未复查按 §14 视为构建失败处理。

已知供应链项：`skia-bindings 0.90.0` 在构建期会从 rust-skia release 下载预编译 Skia 二进制 archive。`Cargo.lock` 只固定 crate source/checksum，不等同于固定该外部 archive；在形成目标发布证据前，必须补充 archive URL/release pin、SHA-256 固定与 CI 校验，并验证离线重建或受控缓存路径。该项不得延伸为安装期、first boot 或运行时下载。

## 9. Machine identity

遵循 library-first 与固定配方（[ADR-0032](adr/0032-device-identity-and-local-credential-lifecycle.md)）：

- `machine-identity` crate 只做候选规范化、placeholder 过滤、2-of-3 判定和 UUID 派生；
- 来源集合固定（DMI system UUID、主板 serial、首块盘 serial）；变更来源集合须修订 ADR-0032，不按证据增量准入新来源库；
- raw source collector 留在 privileged adapter；
- 不把 root/helper framework 泄漏到纯 crate；
- 不提交原始 serial fixture。

## 10. D-Bus 与系统接口

- 使用 typed zbus 或已批准的 Rust binding；
- interface XML、Rust value 和 policy 同步；
- Helper 方法按 capability 划分；
- 不通过 shell 调用 `busctl`/`loginctl` 作为 production 核心路径；
- systemd/logind/desktop 版本差异封装在 adapter；
- 核心 domain 依赖 capability，不依赖 desktop 名称。

## 11. Caddy 和 nFPM

Caddy、nFPM 等外部 release artifact：

- 固定版本；
- 固定官方 source；
- 固定 archive SHA-256；
- 解包后 binary SHA-256；
- 固定 module/feature closure（不引入 encode/brotli 模块；`Accept-Encoding` 透传，压缩在 upstream 完成）；
- 在 CI 验证；
- 不在 postinstall 下载；
- 升级需要 smoke、config compatibility、rollback evidence。

仓库 pin 表示 `REPO-PINNED`，不自动表示目标环境 `ENV-FROZEN`。

## 12. 许可证、漏洞和来源

CI 至少检查：

- Rust advisory、licenses 和 sources；
- Web frozen lockfile/install 与 High/Critical dependency audit；
- pinned external tool 和 package artifact checksum；
- Git history 与当前工作树中的 secret/private key；
- tracked shell script 的 pinned `shfmt` 结果；
- 禁止 dependency/feature；
- OpenAPI 通用规范 lint、项目 contract tests 和生成 artifact clean diff。

高危漏洞处理：

1. 识别是否可达；
2. 建立修复/mitigation；
3. 更新锁文件；
4. 运行完整 contract/package/recovery tests；
5. 记录 evidence；
6. 不因“仅现场使用”忽略可达漏洞。

## 13. 依赖更新批次

更新应按边界分批：

- Rust toolchain；
- Web toolchain；
- protocol/serialization；
- TLS/crypto；
- database；
- GUI；
- Caddy/nFPM；
- test-only。

单个 PR 不应同时升级多个高风险边界，除非为了一个明确 compatibility window。

## 14. 例外

例外必须：

- 有 ADR 或明确 issue；
- 限定路径和时间；
- 有 owner；
- 有风险与 compensating control；
- 有移除日期/触发条件；
- 不放宽 `INV-*`；
- 不以 CI skip 作为长期方案。

过期例外视为构建失败，而不是默认延长。
