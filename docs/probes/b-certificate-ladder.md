# Probe B — Certificate Ladder

> Status: `NOT-RUN`  
> Requirements: `REQ-P0-030`、`REQ-P0-031`、`REQ-P0-032`、`REQ-P0-052`–`REQ-P0-057`  
> Gates: `G0-004`–`G0-009`

## 目标

证明不可跨越的证书阶梯：

```text
server-auth Enrollment
→ Device Identity leaf/chain
→ mandatory-mTLS QUIC
→ active SYNC_STATE
→ Gateway CSR
→ Server-derived Gateway certificate
```

## 环境

```text
COMMIT_SHA=
SERVER_OS=
CLIENT_OS=
PKI_FIXTURE_ID=
WIRE_VERSION=
OWNER=
REVIEWER=
DATE=
```

## 正向路径

1. 生成 Device Identity key/CSR；
2. Enrollment 只返回 Device leaf/chain；
3. 使用 Device certificate 建立 mTLS QUIC；
4. 创建 mock/active `SYNC_STATE`；
5. 提交绑定 command/generation/revision/SPKI 的 Gateway CSR；
6. 检查 certificate SAN/profile/validity 来自 Server Target；
7. 相同 request/SPKI 重试得到同一结果。

## 负向路径

- Enrollment request/response/DB 出现 Gateway material；
- 匿名 QUIC；
- 0-RTT；
- 无 active Command；
- 错误 Device；
- 错误 generation/configuration/assignment；
- CSR 自报未授权 SAN；
- 相同 request 不同 SPKI；
- oversize/unknown wire/invalid CSR；
- 降级到 Enrollment/HTTPS 通用签发。

## 必需证据

- OpenAPI/descriptor/DB fixture；
- TLS handshake/decoder counter；
- certificate inspection；
- idempotency database/result；
- stable ErrorCode；
- redacted logs；
- CI/integration test artifact。

## 结果

```text
STATUS=NOT-RUN
ARTIFACTS=
LIMITATIONS=
```
