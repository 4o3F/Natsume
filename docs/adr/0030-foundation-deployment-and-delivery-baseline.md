# ADR-0030: Foundation, deployment, and delivery baseline

> Status: `ACCEPTED`
> Scope: Natsume V2 engineering foundation, product lifetime, deployment calibration, and package boundary
> Consolidates: ADR-0001, ADR-0003, ADR-0009, ADR-0022
> Supersedes: consolidated historical records; see [`history-map.md`](history-map.md)
> Superseded by: —

## Context

Natsume 同时包含 Rust Server/Client、TypeScript Web、Debian packaging 与验证工具。当前团队、仓库规模和发布关系不足以证明第二套 workspace/build graph 的收益；额外编排层反而会增加依赖图、缓存和供应链边界。

产品会跨赛事复用，但一个已初始化 Server 实例只服务当前一场赛事。为尚不存在的多赛事并发或历史查询需求引入 Event aggregate、运行期切换和跨赛事秘密保留，会扩大所有表、API、权限和恢复流程。

架构复杂度必须按已确认部署事实校准。以下事实是设计输入，不是目标环境已冻结、Gate 已通过或功能已实现的声明。

## Decision

### 原生工具与交付边界

- Cargo 独占 Rust workspace、依赖图和 `Cargo.lock`；pnpm 独占 Web 依赖图和 `pnpm-lock.yaml`。
- `just` 只分发原生命令，不成为第二个依赖解析器、workspace 抽象或 build graph。
- nFPM 只把已构建的固定路径产物映射为 Server/Client Deb；package lifecycle 不编译产品代码、不下载运行时组件、不生成 CA 或私钥。
- package 内容、权限、systemd、D-Bus、XDG 与 Caddy 产物必须能从 manifest 和固定构建输出审计。

### 单赛事生命周期

- 一个已初始化 Server 实例只表示当前赛事，不建立 Event aggregate、跨赛事 runtime phase 或历史赛事产品模型。
- 跨赛事复用通过显式、破坏性的 single-lifetime reset 完成；reset 必须按恢复边界清除业务状态与秘密，不能通过 nullable `event_id` 或运行期数据库切换逐步引入多赛事模型。

### 部署事实与信任假设

任一条失效时，必须先修订本 ADR，再调整依赖机制。

#### 环境与规模

- **F1**：约 500 台异构受管工作站；非统一采购，v1 曾发生 MAC 地址冲突。
- **F2**：单机房有线 LAN，由第三方管理；带宽受限，必须节约；UDP 通过性无保证。
- **F3**：单 Server 实例；Server 地址**由部署时配置**，不是产品内固定的 IP literal，但在同一部署内必须保持稳定（TLS leaf 的 IP-SAN 与全部 Client endpoint 都绑定该值，更换需重新签发与重配）。工作站使用 DHCP 短租期，不能保证静态 IP 或长租期。
- **F4**：基础 OS 镜像派生自 ICPC 官方镜像；大版本更新可能改变桌面栈；当前周期为 GNOME + X11；最终镜像由本项目构建。
- **F5**：DOMjudge 是外部竞赛系统，其 Web Server 已启用 brotli；v1 实测其相对 gzip 的带宽收益显著，这是保留本机 HTTPS 的依据。
- **F6**：一次部署服务一场竞赛；产品跨赛事长期复用。
- **F7**：开发窗口约 6 个月，团队 3 人。
- **F8**：操作员 1–3 人且互相信任；不存在并发导入场景；权限只需固定 `admin` 与 `viewer` 两级。
- **F9**：审计仅面向赛事管理员，不对外提交。
- **F10**：Home 在热身赛后和赛前连续测试中多次重置；初始 Home 不得烤入镜像。

#### 信任假设

- **T1**：赛前存在物理受控的 provisioning window；窗口内操作员受信，关闭后本机使用者不受信。
- **T2**：选手是非 root 本地用户；本地 root、物理攻击和固件篡改不在防护范围。
- **T3**：选手不知道自己的 DOMjudge 凭据；登录必须由系统代为完成。
- **T4**：venue LAN 可能存在未授权设备；跨网线流量视为可嗅探，Server 与 DOMjudge 身份必须密码学可验证。

## Alternatives

- Nx、Turborepo、Bazel 或通用跨语言编排：当前收益不足以覆盖第二套图、插件、缓存语义和迁移成本。
- 多仓库：增加协议、版本、发布和文档一致性成本。
- 自定义 dpkg staging、Cargo/npm 主导 packaging 或容器交付：无法形成清晰、可审计的工作站系统包边界。
- 多 Event、软归档或运行期数据库切换：保留更多秘密并扩大生命周期状态机。
- 按未陈述的最坏情况设计：此前已经产生与 F1～F10 不相称的证书、并发和桌面机制。

## Consequences

### Positive

- 依赖图、lockfile、构建与 package ownership 清晰；CI 与本地命令更易审计。
- 领域模型、权限、reset 和秘密保留边界保持小而明确。
- 每个安全与复杂度取舍都有可引用的事实基础。

### Negative / trade-offs

- 跨语言增量缓存较弱，`just` recipe 和 nFPM lifecycle 仍需显式维护与 smoke evidence。
- Server 实例不能查询历史赛事；复用依赖 destructive reset 与备份/恢复纪律。
- 部署形态、团队或信任模型变化时，需要先重审本 ADR 及所有受影响主题。

## Acceptance basis and revisit trigger

F1 的依据包括 v1 MAC 冲突事故，F5 的依据包括 v1 brotli/gzip 实测。目标环境、package lifecycle 和 support claim 的证据仍由平台与 Gate 文档管理。

当仓库规模、构建时间或跨语言发布关系有测量证据证明新编排器能降低总复杂度，nFPM 无法表达冻结的 Debian lifecycle，出现正式多赛事需求，或 F1～F10/T1～T4 任一变化时重开。

## Normative sources

- [Architecture](../architecture.md)
- [Domain model](../domain-model.md)
- [Repository layout](../repository-layout.md)
- [Dependency policy](../dependency-policy.md)
- [Security and recovery](../security-recovery.md)
- [Supported platform](../supported-platform.md)
- [Roadmap](../roadmap.md)
