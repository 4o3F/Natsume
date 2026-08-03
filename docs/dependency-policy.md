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
- ErrorCode crate 不依赖 Axum、Prost、zbus 或 SQLx。

## 4. 加密与秘密

允许的选择必须来自经过维护的 Rust crypto ecosystem，并使用公开审计的构造。当前基线包括 AEAD、HKDF、SHA-2、secrecy/zeroize 等类型。

禁止：

- 自制 cipher/MAC/KDF；
- 复用 nonce；
- 把 Machine ID 当作 key；
- 在 serde/debug 中暴露 secret；
- 通过 env、argv、unit text 或普通配置传 secret；
- 依赖外部 shell 工具完成核心加密；
- 为方便测试禁用证书验证。

依赖升级必须运行已知向量、wrong-key、tamper 和 crash recovery 测试。

## 5. TLS 与控制通道

全部入口为 server-auth TLS（[ADR-0033](adr/0033-enrollment-and-device-control-boundary.md)）：Operator HTTP、Enrollment 与 Device WSS 共用同一 rustls 栈与同一 TCP 端口，不引入第二个协议栈（无 quinn/QUIC、无独立 HTTP/2 gRPC 栈）。

必须显式：

- trust roots；
- server name/IP-SAN；
- ALPN；
- protocol version；
- 0-RTT/early data 保持关闭；
- WS subprotocol version；
- max frame/connection limits；
- 签发侧的 certificate profile 与 key usage（Gateway）。

测试 helper 不能被 production build 导出为 dangerous verifier。Device Token 比对必须常数时间。

## 6. SQL 和持久化

- SQLx/migration 由 owning module 使用；
- domain 不依赖 SQL row type；
- query 使用参数绑定；
- migration 从空库和升级路径测试；
- 不引入第二个 ORM/数据库层；
- SQLite WAL 是初始部署选择，不在 domain API 中暴露；
- backup/restore 通过 runbook 和 integration test 验证。

## 7. Web 依赖

Web 只通过 pnpm workspace 管理。

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

- Rust advisory；
- Rust licenses/sources；
- Web lockfile/install；
- package artifact checksum；
- secret/private key；
- 禁止 dependency/feature；
- 生成 artifact clean diff。

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
