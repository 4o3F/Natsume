# ADR-0024: DOMjudge auto-login via X-Headers injection

> Status: `ACCEPTED`  
> Scope: Natsume V2 数据面凭据注入  
> Supersedes: —  
> Superseded by: —

## Context

选手不知晓自己的 DOMjudge 凭据（[ADR-0022](0022-deployment-facts-and-trust-assumptions.md) T3），登录必须由系统代为完成。DOMjudge 原生提供两类免输入认证：按源 IP 认证（`ipaddress`）与 X-Headers 认证（`xheaders`，向 `/login` 发送 `X-DOMjudge-Login` 与 base64 的 `X-DOMjudge-Pass`）。工作站为 DHCP 短租期、无法保证静态 IP（F3），IP 认证不可行。

Caddy 原生支持按路由注入固定请求头（`header_up`），无需自研模块即可实现 xheaders 注入；但注入头中的密码为 base64（等同明文），且 Caddy → DOMjudge 一段走可嗅探的共享网络（T4）。

## Decision

1. DOMjudge 侧启用 `xheaders` 认证方法（`auth_methods` 配置）；这是 **DOMjudge 侧部署前提**，Natsume 不拥有其配置，但在 contract lab 与平台冻结中验证。
2. Device Daemon 将本 Seat 凭据渲染进 Caddy 配置中**仅匹配 `/login` 路由**的 `header_up X-DOMjudge-Login` / `header_up X-DOMjudge-Pass`（base64）；不做全站注入。
3. 含凭据的 Caddy 配置文件是 **secret artifact**：`0640 root:natsume-gateway`，纳入 [security-recovery.md](../security-recovery.md) 秘密清单；其内容不进入日志、audit、指标或错误链。
4. 设备侧凭据真相源为 `SYNC_SECRET` 落盘的凭据文件（`0600 root:root`）；Caddy 配置为**派生物**，rebind 或凭据变更后由 Daemon 重渲染并原子激活。
5. **Caddy → DOMjudge upstream 必须为 TLS**，至少覆盖 `/login`；该要求为不变量 `INV-DATAPLANE-02`，本机 loopback HTTPS 不能替代它——密码走的是机房网线，不是 loopback。
6. Caddy 保持 `Accept-Encoding` 透传（不配置 `encode`），brotli 压缩发生在 DOMjudge web server（F5）；透传行为与 upstream brotli 启用状态进入 contract lab 验证项。

## Alternatives

- **DOMjudge IP 认证**：DHCP 短租期下不可行（F3）。
- **自研 Caddy 登录模块**（表单 CSRF + POST + cookie 管理）：与固定 module closure 供应链策略冲突，且随 Caddy/DOMjudge 升级持续维护。
- **Daemon 充当表单登录 broker**：可行但机制多于 xheaders（CSRF 解析、cookie 转发）；DOMjudge 已提供官方免表单路径时无必要。
- **HTTP Basic 注入**：DOMjudge 团队 Web 界面对 Basic 的支持未经验证，且无官方文档背书。

## Consequences

### Positive

- 无自研 Caddy 模块，Caddy 保持哑的、pin 死的反向代理；
- 密码消费者收敛为 Daemon（渲染）与 Caddy（仅 `/login` 注入）两处，路径可枚举；
- 登录时序无状态：会话过期后浏览器再次访问 `/login` 即重新认证。

### Negative / trade-offs

- 凭据以可逆形式存在于 Caddy 配置文件（由文件权限与 T2 保护，与设备凭据文件同级）；
- 对 DOMjudge `auth_methods` 配置产生部署耦合，版本升级需回归验证；
- upstream TLS 成为硬性部署前提，DOMjudge 侧证书信任需在平台冻结中一并解决。

## Evidence and revisit trigger

- 接受前需要：冻结 DOMjudge 版本上 xheaders 登录的可复现验证；`/login` 之外路由无注入头的负向验证；upstream 非 TLS 时拒绝激活（`INV-DATAPLANE-02`）的负向验证。
- 重开条件：DOMjudge 移除或变更 xheaders 契约；或出现选手需要自持凭据的赛制要求。

## References

- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
- DOMjudge manual: X-Headers authentication（版本以 [supported-platform.md](../supported-platform.md) 冻结为准）
