# Natsume V2 系统架构

> 状态：`NORMATIVE`  
> 适用范围：Natsume V2  
> 当前实现成熟度：Phase 0 工程基线  
> 校准基线：[ADR-0022](adr/0022-deployment-facts-and-trust-assumptions.md)  
> 相关文档：[领域模型](domain-model.md) · [边界契约](contracts.md) · [状态与执行](state-and-execution.md) · [安全与恢复](security-recovery.md)

## 1. 目标

Natsume 为单场竞赛现场提供以下能力：

1. 从固定 CSV 导入 Seat、account 和 password；
2. 将 Seat 绑定到受管理工作站；
3. 以显式命令同步非秘密状态和密码；
4. 在 provisioning 窗口内为 Device 签发 Token 与 Gateway certificate；
5. 在工作站本地提供受控浏览器数据面与 DOMjudge 自动登录；
6. 编排受管桌面会话和 Home 准备；
7. 让操作员看到 Target、Observed、Drift、Command 和审计记录；
8. 在身份、凭据、证书或 Home 无法证明安全时停止敏感操作。

架构优先级依次是：

1. 身份和秘密安全；
2. 可审计的显式副作用；
3. 离线稳态和可恢复性；
4. 低耦合、高内聚；
5. 可验证、可打包和可运维；
6. 与 [ADR-0022](adr/0022-deployment-facts-and-trust-assumptions.md) 部署事实相称的实现规模。

## 2. 产品范围

### 2.1 包含

- 一个 Server 实例服务当前一场竞赛；
- 当前单场竞赛的一份可重复 import 的 confirmed contest configuration；
- `seat,account,password` CSV；
- Device 注册（provisioning 窗口）、绑定、配置和状态；
- Device Token 与 Gateway certificate；
- operator Web Panel（admin/viewer 两级角色）；
- Caddy 到 DOMjudge 的本地 HTTPS 数据面与 `/login` 自动登录注入；
- Session Agent、受管会话和 Home 准备；
- 审计、命令状态和恢复流程。

### 2.2 不包含

- 多 Event、跨赛事历史业务模型或运行时 phase；
- XLSX/ODS 或任意列映射；
- 密码导出；
- Device merge/split；
- 通用远程 shell、文件管理或任意 systemd unit 控制；
- 任意反向代理配置平台；
- Server 高可用或多控制器一致性；
- ACME、TOFU、运行时下载或 postinstall 下载；
- 多桌面环境同时支持（每周期单镜像，[ADR-0027](adr/0027-single-image-desktop-cycle.md)）；
- 可编辑的角色/权限模型（固定 admin/viewer）；
- 对本地 root、物理攻击者或固件篡改的防护；
- 将 Session lock 或遮罩 UI 当作网络隔离/完整性边界。

需要进入这些范围时，必须先新增 ADR，而不是在现有接口中加入特例。

## 3. 系统上下文

```mermaid
flowchart LR
    Operator[Operator Browser]
    Server[natsume-server]
    Web[Web Panel]
    Device[natsume-device-daemon]
    Helper[natsume-privileged-helper]
    Agent[natsume-session-agent]
    Caddy[Caddy]
    Browser[Managed Browser]
    DOMjudge[DOMjudge]

    Operator -->|HTTPS| Server
    Server --> Web
    Device -->|Enrollment HTTPS| Server
    Device <-->|WSS + Device Token| Server
    Device -->|typed D-Bus| Helper
    Agent <-->|local typed D-Bus| Device
    Device -->|rendered config + reload| Caddy
    Browser -->|loopback HTTPS| Caddy
    Caddy -->|fixed TLS upstream| DOMjudge
```

该图表示信任和调用方向，不表示所有组件已经实现。Operator HTTP、Enrollment 与 Device WSS 共用同一 TCP 端口（[ADR-0023](adr/0023-wss-control-channel-with-device-token.md)）。

## 4. 进程与职责

### 4.1 `natsume-server`

拥有：

- operator 身份、授权（admin/viewer）和 HTTP API；
- CSV staging、preview、commit；
- Server truth；
- Target 计算；
- Device lifecycle 和 binding；
- provisioning 窗口状态；
- Enrollment：Device Token 与 Gateway certificate 签发（origin CA key 保管）；
- WSS Device control；
- Command dispatcher；
- Server vault；
- AuditEvent；
- Web Panel 静态资源或集成入口。

不得：

- 直接访问工作站本地文件、桌面或 Caddy；
- 把 password 明文、private key 或 Device Token 值加入 Target、Observed、普通 API/Browser 可见响应、日志、指标、audit diff 或导出；
- 将 Web request 生命周期当作远端副作用完成边界；
- 在 provisioning 窗口之外签发任何 token 或证书。

内部模块边界见 [仓库布局](repository-layout.md)。

### 4.2 Web Panel

拥有：

- operator 交互；
- Preparation Center；
- Device、binding、Target、Observed、Drift、Command 和 Audit 视图（轮询刷新）；
- 人工触发 `SYNC_STATE`、`SYNC_SECRET`、session/home 操作；
- provisioning 窗口开关入口；
- 可访问性和错误呈现。

不得：

- 保存密码到浏览器持久化存储；
- 自行计算权限、Target 或 Drift；
- 本地重算 import diff classification；只渲染 Server 返回的结构化 preview；
- 解析错误显示文本作业务判断；
- 宣称命令已完成，除非 Server 状态已经确认。

### 4.3 `natsume-device-daemon`

拥有：

- identity-before-credentials 启动检查；
- Client 凭据文件（Device Token、Gateway key/leaf、Seat 凭据、LKG）；
- Enrollment 客户端（含 Gateway CSR）；
- WSS 控制连接；
- Command journal 和幂等执行；
- Target 应用和 Observed 采集；
- Caddy 配置渲染（含 `/login` 自动登录注入）、validate、原子激活与 LKG 回滚；
- Session Agent 协调；
- Home transaction 编排；
- 离线稳态。

不得：

- 直接执行网络输入给出的路径、UID、unit、命令或 upstream；
- 把一个传输 handler 同时作为凭据、Caddy 和 Home 的业务实现；
- 将密码返回给 Server、Agent、浏览器或普通日志；
- 在身份不确定或凭据文件损坏时自动 re-enroll 或重建凭据。

内部必须分离 transport、application、domain、port 和 adapter。见 [仓库布局](repository-layout.md)。

### 4.4 `natsume-privileged-helper`

拥有最小 root 权限能力，例如：

- 受限硬件标识采集；
- 固定 contest user 和 Home backend 所需的受限系统操作；
- 由封闭枚举定义的少量特权动作。

不得：

- 建立外部网络连接；
- 持有 DOMjudge 密码、Device Token 或 Gateway private key；
- 接受任意 shell、任意路径、任意 UID、任意 unit 或任意环境变量；
- 读取 Server vault 或 Client 凭据文件；
- 代替 Device Daemon 作业务决策。

Helper 的每个方法必须是独立、可审计、参数封闭的 capability。

### 4.5 `natsume-session-agent`

拥有：

- 由系统级 XDG Autostart 在当前图形会话中直接启动；
- 当前会话资格和 singleton 验证；
- typed snapshot 的本地展示（view kind 为封闭 enum，可经版本升级扩展）；
- Seat/binding 提示；
- lock/unlock 等经授权的会话交互；
- focus-denied 等 UI 结果报告。

不得：

- 使用 systemd user unit；
- 使用 bootstrap/run 两阶段或环境转交文件；
- 读取凭据文件、密码、Device Token 或 Gateway private key；
- 管理 Caddy；
- 调用 Server；
- 依赖外部 GUI helper 或 runtime UI interpreter。

### 4.6 Caddy

拥有：

- package-pinned binary 和固定 module closure；
- loopback HTTPS（Gateway certificate）；
- BLOCKED 状态页；
- READY 时代理固定 DOMjudge upstream（TLS）；
- 仅 `/login` 路由的 X-Headers 自动登录注入（[ADR-0024](adr/0024-domjudge-autologin-via-xheaders.md)）；
- `Accept-Encoding` 透传（brotli 在 upstream 完成）。

不得：

- 决定 Device 身份、binding、授权；
- 接收自由格式 upstream、路径或配置片段；
- 因 Session lock/unlock 变更配置；
- 在证书或配置未验证时代理 upstream；
- 在 `/login` 之外的路由注入凭据头。

### 4.7 Managed Browser

拥有：

- 访问固定 loopback HTTPS origin；
- 竞赛现场允许的浏览器策略。

不得：

- 直接访问 Device control 或凭据文件；
- 绕过 Caddy 访问由 Natsume 管理的 upstream；
- 被视为秘密存储。

### 4.8 DOMjudge

是外部竞赛系统。Natsume 只依赖已冻结的访问契约，不拥有其用户、比赛或认证实现。部署前提（在平台文档冻结并经 contract lab 验证）：版本与 endpoint；`auth_methods` 含 `xheaders`；web server 启用 brotli；upstream 至少 `/login` 为 TLS。

## 5. 信任边界

| 边界 | 认证 | 数据类型 | 失败策略 |
|---|---|---|---|
| Operator → Server | operator session/固定角色 | 人类控制面 | 拒绝并审计 |
| Device Enrollment → Server | server-auth HTTPS + provisioning 窗口 | 硬件身份证据、Gateway CSR | 窗口关闭或校验失败即拒绝，零状态变更 |
| Device control ↔ Server | server-auth TLS + Device Token（WSS） | typed protocol、Command、Observed | 认证失败 401（解码前）；协议失败断开 |
| Device Daemon → Helper | 本地 system D-Bus + OS policy | 封闭特权请求 | 拒绝且不降级 |
| Agent ↔ Device Daemon | 本地 session-aware typed IPC | UI snapshot 和会话动作 | lease/epoch 失效 |
| Browser → Caddy | loopback HTTPS | 页面和 DOMjudge 流量 | BLOCKED/503 |
| Caddy → DOMjudge | 固定 TLS upstream（`INV-DATAPLANE-02`） | 竞赛数据面与 `/login` 凭据注入 | 不健康或非 TLS 则 fail closed |

信任边界之间不得共享"全能 context"或未分类秘密。

## 6. 分层与依赖方向

每个有业务逻辑的进程应采用以下方向：

```text
transport / presentation adapters
              ↓
application use cases
              ↓
domain policies and value objects
              ↓
ports
              ↓
database / credential / protocol / OS adapters
```

规则：

1. domain 不依赖 Axum、SQLx、zbus、Slint 或 Caddy；
2. application 不暴露数据库 row、Protobuf message 或 D-Bus object；
3. adapter 负责结构转换和公开错误映射；
4. transport handler 只完成认证、解码、调用 use case 和编码；
5. 跨模块调用使用明确 port 或 command，不直接跨表写入；
6. composition root 可以依赖所有模块，但不得包含业务规则；
7. shared crate 只承载稳定、至少两个生产消费者使用的契约。

## 7. 数据所有权

| 数据 | 唯一 Owner | 允许消费者 |
|---|---|---|
| confirmed contest configuration / current Seat collection | contest-domain | Target、Web、CSV |
| account 标识 | contest-domain | Target、Web |
| password 明文 | Server vault / Client 凭据文件的短生命周期 use case | secret sync、自动登录配置渲染 |
| Device lifecycle 与 provisioning 窗口 | identity-enrollment | Web、Target |
| Device Token（哈希） | identity-enrollment | WSS 认证 adapter |
| Binding | contest-domain | Target、session |
| Target | configuration-target | dispatcher、Web |
| Observed snapshot | device-control | Drift、Web |
| Command | command-dispatch | Web、audit |
| Server certificate/key | pki | Server TLS adapter |
| origin CA key | pki | Enrollment 签发 use case |
| Gateway certificate/key | Client 凭据文件 | Caddy 配置渲染 |
| Machine Hardware ID | identity startup | Enrollment、Observed |
| Session/Home epoch | local runtime domain | Agent/Home adapters |
| AuditEvent | audit module | operator query/export |

数据库表是模块实现细节。一个模块不得通过任意 SQL 写入另一个模块拥有的状态。

## 8. 关键业务流程

### 8.1 CSV 到 Server truth

```text
upload（全局单 pending candidate）
  → encrypted staging
  → strict parse
  → immutable candidate import
  → server-computed redacted preview（opaque preview_token）
  → explicit Import Commit
  → 双 CAS（ContestConfigurationRevision + AssignmentRevision）
  → atomic unbind-and-replace（仅当存在待解绑 bound Seat）
  → redacted AuditEvent
  → Target 可重算（仅 material 变化时）
```

每个 CSV 都是完整的 contest configuration candidate。**Material** Import Commit 才替换 confirmed contest configuration；**no-op**（`is_noop`）只记录 lineage 与 redacted AuditEvent，不提升 revision、不触发 Target churn。Import Commit 不创建 Command，不产生 Device I/O。任一 CAS 失配须重新 preview，且不改变 confirmed truth。权威规则见 [领域模型](domain-model.md) 与 [ADR-0028](adr/0028-single-operator-import-and-secret-evidence-scope.md)。

### 8.2 Device Enrollment（provisioning 窗口内）

```text
identity-before-credentials（ADR-0025 配方）
  → server endpoint/trust validation（预置 CA + IP-SAN）
  → 本地生成 Gateway keypair + CSR
  → server-auth HTTPS enrollment 请求
  → Server：窗口门禁 → 校验 → 同一事务签发 { Device Token + Gateway leaf } + 审计
  → Client 校验响应（SPKI/chain/SAN）
  → 本地原子持久化（token 0600、gateway 0640）
  → WSS 控制连接（Bearer token）
```

窗口关闭后 Server 拒绝一切签发。重复 Enrollment 为受审计的替换。

### 8.3 非秘密状态同步

```text
operator starts SYNC_STATE
  → Target snapshot 与派生代际被冻结
  → durable Command is created
  → Device persists receipt
  → Device validates and stages state
  → Caddy 配置渲染 → validate → 原子激活（失败回滚 LKG）
  → Observed snapshot
  → Command terminal status
```

`SYNC_STATE` 不涉及任何签发。

### 8.4 密码同步

```text
operator starts SYNC_SECRET
  → current assignment + credential revision are frozen
  → secret read from Server vault
  → encrypted authenticated command（WSS）
  → Device validates current binding/revision
  → 凭据文件原子更新
  → Caddy /login 注入配置重渲染并原子激活
  → secret is discarded from transient buffers
  → redacted terminal status
```

没有自动 secret sync。

### 8.5 Session/Home

```text
current binding and epochs
  → prepare Home transaction
  → prove backend result
  → start/validate graphical session
  → XDG Autostart starts Agent
  → Agent validates current logind session
  → typed UI snapshots and actions
```

Home 无法证明安全时不得启动受管 session。Session lock/unlock 不改变 Caddy。

## 9. 部署拓扑

### 9.1 Server package

包含：

- `natsume-server`；
- Web assets；
- systemd unit、sysusers/tmpfiles、配置目录；
- migration 和必要静态契约。

Server control certificate 由离线控制根或经批准的离线流程签发。postinstall 不生成 CA/private key，也不下载运行时组件。

### 9.2 Client package

包含：

- Device Daemon；
- Privileged Helper；
- Session Agent；
- 固定 Caddy binary；
- system service、D-Bus policy；
- `/etc/xdg/autostart/org.natsume.SessionAgent.desktop`；
- BLOCKED 状态页静态资源；
- debconf 预置：Server endpoint、Gateway hostname、站点参数。

Client 包不得包含 Session Agent systemd user unit。

## 10. 可用性与离线稳态

Device 可以在 Server 暂时不可达时继续使用已经验证的本地状态，但不得推断新授权：

- 已验证 LKG 配置可以继续服务；
- 已安装且未过期的 Gateway certificate 可以继续使用；
- 当前有效 binding 的本地凭据可以继续使用；
- 不得在离线时创建新 binding、获得新 token/证书或接受陈旧 revision；
- 重连后通过 Observed 和 Drift 收敛；
- 本地损坏不能通过"自动重建身份"绕过。

## 11. 变更治理

低耦合/高内聚自检、应拒绝的设计信号，以及需要或不需要 ADR 的变更分类，见 [`CONTRIBUTING.md`](../CONTRIBUTING.md)。
