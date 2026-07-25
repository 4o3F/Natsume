# Probe C — Caddy 与 DOMjudge

> Status: `NOT-RUN`  
> Requirements: `REQ-P0-003`、`REQ-P0-060`、`REQ-P0-066`  
> Gates: `G0-010`、`G0-012`、`G0-013`

## 目标

证明固定 Caddy artifact、loopback HTTPS、BLOCKED 503、typed activation、Gateway certificate 验证、固定 DOMjudge upstream 和 LKG/rollback。

## 环境

```text
COMMIT_SHA=
CLIENT_OS=
CADDY_VERSION=
ARCHIVE_SHA256=
BINARY_SHA256=
MODULE_LIST=
DOMJUDGE_VERSION=
UPSTREAM=
OWNER=
REVIEWER=
DATE=
```

## 步骤

1. 验证 official source、archive/binary checksum 和 modules；
2. 安装 Client package，确认 Caddy 非 root 和 Admin boundary；
3. 未配置时访问主页面：503 + 本地 BLOCKED 页面；
4. 检查 CSP、local assets、allowlist JSON、`textContent`；
5. 注入 free-form error/HTML/secret-like data，确认拒绝或不呈现；
6. 正确 Gateway cert/config 进入 READY；
7. 错 key、SAN、profile、expiry、upstream/config 不能 READY；
8. Caddy validate/load 失败保留 LKG 或 BLOCKED；
9. DOMjudge 正常/超时/拒绝/证书失败；
10. 保存 config hash、status、HTTP、Caddy log 和 package evidence。

## 不可出现

- `session_locked`；
- password、private key、path、free-form stack；
- arbitrary upstream/config fragment；
- 未验证候选配置代理流量；
- runtime download。

## 结果

```text
STATUS=NOT-RUN
ARTIFACTS=
LIMITATIONS=
```
