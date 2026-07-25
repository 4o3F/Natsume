# Gateway Certificate Sync

> 适用：`SYNC_STATE` 等待、失败或更新 Gateway certificate  
> 关键不变量：`INV-CERT-01`、`INV-CERT-02`、`INV-DATAPLANE-01`

## 1. 前提

- Device mTLS connection authenticated；
- active `SYNC_STATE` Command 已存在；
- Device、assignment revision、configuration generation 与当前 Target 一致；
- Server Gateway issuer 可用；
- Client vault 可写；
- Caddy 尚未使用未验证候选 certificate。

Enrollment 页面或通用 operator API 不是 Gateway 签发入口。

## 2. 诊断上下文

记录：

```text
DEVICE_PK
COMMAND_ID
ASSIGNMENT_REVISION
CONFIGURATION_GENERATION
REQUEST_ID
SPKI_FINGERPRINT
TARGET_HOSTNAME
EXPECTED_PROFILE
CURRENT_GATEWAY_SERIAL
ERROR_CODE
```

不要记录 private key、CSR private material 或完整 vault。

## 3. 正常流程

1. Device 本地生成/选择 Gateway key；
2. 生成 CSR 和 SPKI fingerprint；
3. 在 active Command 的 authenticated QUIC 上发送 request；
4. Server 验证 Device、Command、generation、revision 和 Target；
5. Server 忽略 CSR SAN，按 Target 派生 SAN/profile/validity；
6. Server 进行 request/SPKI 幂等 lookup；
7. 签发或返回既有结果；
8. Device 验证 key match、chain、SAN、profile、validity；
9. 原子保存 certificate；
10. 继续 Caddy staging/validation/activation；
11. 发送 terminal status 和 Observed。

## 4. 失败分流

### 无 active Command

- 检查 `SYNC_STATE` 是否终态/取消/过期；
- 不创建通用 certificate request；
- 刷新 Target 后创建新的 state sync。

### stale generation/revision

- 不复用旧 CSR 授权；
- 刷新 Server truth/Target；
- 创建新的 `SYNC_STATE`；
- 旧 request 保留审计。

### 相同 request 不同 SPKI

- 视为 conflict；
- 停止当前 Command；
- 检查 journal、重试实现和潜在 key replacement；
- 不得选择任一 key “继续”。

### CSR SAN 与 Target 不同

- Server 应忽略 CSR SAN；
- inspection 必须显示 certificate 使用 Target SAN；
- 若 Server按 CSR 签发，立即停止并标记安全失败。

### certificate validation 失败

- 不写入 active vault namespace；
- 不 reload Caddy；
- 保留旧 LKG 或 BLOCKED；
- 检查 issuer/profile/time/key match。

### issuer unavailable

- Command 按 retry/expiry policy；
- 不得切换 Enrollment 或本地自签；
- 现有有效 LKG 可继续，过期/无效则 BLOCKED。

## 5. 幂等验证

对同一 request/SPKI 重试：

- serial/certificate result 应相同；
- 不能额外签发；
- Server/Device audit 能关联。

改变 SPKI 后重试同一 request：

- 必须 conflict；
- 不能覆盖既有 key/certificate。

## 6. 成功判定

- certificate SAN/profile 来自 Target；
- private key 只在 Client；
- serial/SPKI/validity 已记录；
- Command/Observed generation 匹配；
- Caddy 只在完整验证后 READY；
- Enrollment 仍无 Gateway material；
- audit 完整。

## 7. Evidence

```text
COMMAND_ID=
REQUEST_ID=
SPKI_SHORT=
TARGET_HOSTNAME=
CERT_SERIAL=
CERT_SAN=
CERT_PROFILE=
IDEMPOTENT_RETRY=
SPKI_CONFLICT_TEST=
CADDY_RESULT=
OBSERVED_SEQUENCE=
AUDIT_EVENT=
OWNER=
REVIEWER=
```
