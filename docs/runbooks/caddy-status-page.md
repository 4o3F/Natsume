# Caddy Status Page and Data Plane Recovery

> 适用：Caddy BLOCKED/READY 异常、503 页面异常、config load/upstream 故障  
> 关键不变量：`INV-SECRET-01`、`INV-INPUT-01`、`INV-DATAPLANE-01`、`INV-SESSION-01`

## 1. 记录

```text
DEVICE_PK
CONFIGURATION_GENERATION
ASSIGNMENT_REVISION
CADDY_VERSION
BINARY_SHA256
MODULE_LIST
CADDY_STATUS
CONFIG_HASH
LKG_HASH
GATEWAY_CERT_SERIAL
GATEWAY_CERT_EXPIRY
UPSTREAM_ID
ERROR_CODE
```

不收集 password、private key、完整 vault 或自由格式 secret payload。

## 2. BLOCKED 是安全状态

当 certificate、config、Target 或 upstream 前置无法证明时，保持 BLOCKED：

- 主页面 HTTP 503；
- 本地静态资源；
- allowlist 状态；
- 严格 CSP；
- 动态值 `textContent`；
- 不代理 upstream。

不要为了可用性手工修改 Caddyfile、关闭 TLS 验证或指向临时 upstream。

## 3. 状态页异常

检查：

1. package static assets/hash；
2. 本地 loopback HTTPS；
3. CSP/header；
4. allowlist JSON schema；
5. 主页面 status code；
6. 无 `session_locked`；
7. 无 password/path/free-form stack；
8. HTML/script injection 不能执行；
9. 离线时仍可加载；
10. 页面内容与 Device typed state一致。

若状态页本身失败，数据面仍保持 BLOCKED；可以通过 system logs/Observed 诊断。

## 4. READY 前置

逐项验证：

- current Target/generation；
- Gateway private key/leaf match；
- chain/trust；
- SAN/hostname；
- profile/EKU；
- validity/time；
- fixed upstream；
- 完整 candidate config；
- Caddy validate；
- Admin socket权限；
- load成功；
- loopback health；
- LKG写入；
- Observed。

缺一项不得 READY。

## 5. Config load 失败

1. 保留 candidate config hash和错误；
2. 确认 active config是否仍为已验证 LKG；
3. 若是，继续 LKG并报告 Drift/error；
4. 若不能证明，切/保持 BLOCKED；
5. 修复 generator/Target/certificate；
6. 创建新的 `SYNC_STATE`，不手工 load；
7. 验证原失败可复现并加入测试。

## 6. Gateway certificate 问题

转 [Gateway certificate sync](gateway-certificate-sync.md)。不得：

- 在 Enrollment获取 Gateway leaf；
- 自签临时 leaf；
- 忽略 SAN/expiry；
- 复制其他 Device key/certificate；
- 通过 browser warning继续。

## 7. Upstream 问题

- 核对冻结 DOMjudge endpoint；
- 检查网络、TLS、健康契约；
- 不接受 operator自由输入临时 URL；
- 根据冻结 policy维持 READY-with-health或BLOCKED；
- 若 policy尚未冻结，保持 BLOCKED并记录 `BLOCKED-INPUT`；
- 修复 DOMjudge后验证数据面，不修改 session状态。

## 8. Session 解耦检查

若问题发生在 lock/unlock 周围：

- 比较 Caddy Admin call count、hash、generation、status；
- 任何变化视为缺陷；
- 不要把 session状态加入 BLOCKED payload；
- 转 [Session lock recovery](session-lock-recovery.md)。

## 9. 成功判定

- 正确版本/checksum/modules；
- 状态页安全；
- 证书/config/upstream验证；
- Caddy状态与Observed一致；
- LKG可恢复；
- 无任意配置/secret；
- Session操作不影响数据面。

## 10. Evidence

```text
STATUS_BEFORE=
STATUS_AFTER=
CONFIG_HASH_BEFORE=
CONFIG_HASH_AFTER=
LKG_HASH=
CERT_SERIAL=
CERT_VALIDATION=
CADDY_VALIDATE=
CADDY_LOAD=
HTTP_STATUS=
CSP_TEST=
INJECTION_TEST=
UPSTREAM_TEST=
OBSERVED_SEQUENCE=
OWNER=
REVIEWER=
```
