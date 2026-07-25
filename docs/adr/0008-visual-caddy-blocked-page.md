# ADR-0008: Visual Caddy BLOCKED page

> Status: `ACCEPTED`  
> Scope: Natsume V2

## Context

工作站在未配置、证书失败或上游不可用时需要向现场人员显示有限状态。纯连接失败难以诊断，但自由格式错误页可能泄密或产生 XSS。

## Decision

Caddy 在非 READY 时提供 package-contained visual BLOCKED 页面，主入口返回 503。页面只消费 allowlist JSON/typed state，严格 CSP，动态值通过 `textContent`；不显示 secret、路径、free-form error 或 `session_locked`。

## Alternatives

- 浏览器连接失败：现场可诊断性差。
- Server 托管错误页：Server 离线时不可用。
- 自由格式 HTML/错误：安全和稳定性不可控。

## Consequences

### Positive

- 本地、离线可诊断；
- 数据面保持 fail closed；
- 状态内容可测试。

### Negative / trade-offs

- 需要维护静态资源和有限状态映射；
- 不能展示所有内部细节。

## Evidence and revisit trigger

Probe C/E 和状态页安全测试必须证明 503、CSP、allowlist、无 secret 和 no upstream proxy。

## References

- [state-and-execution.md](../state-and-execution.md)
- [caddy-status-page.md](../runbooks/caddy-status-page.md)
