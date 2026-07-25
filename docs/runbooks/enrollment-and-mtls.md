# Enrollment and mTLS

> 适用：新 Device 首次注册或经批准的重新注册  
> 关键不变量：`INV-IDENTITY-01/02`、`INV-CERT-01/02`

## 1. 前提

- 目标 Machine identity 状态为首次允许或经批准的新生命周期；
- identity file/vault 不存在冲突；
- Server endpoint、trust 和 IP-SAN 已冻结；
- Server control certificate 有效；
- Device issuer 可用；
- 时间同步；
- Device package/version 支持当前 wire；
- replacement 场景先执行 [Device replacement](device-replacement.md)。

## 2. Client 启动检查

1. 启动 Device Daemon；
2. 检查 Machine Hardware ID result；
3. 确认 identity-before-vault；
4. 若 identity 不可用/冲突/不匹配或 vault decrypt failure，停止并转 [恢复 runbook](machine-identity-and-vault-recovery.md)；
5. 确认 endpoint/trust；
6. 不删除 identity/vault 继续。

## 3. Enrollment

1. Client 本地生成 Device Identity private key；
2. 生成 Device Identity CSR；
3. 通过 server-auth HTTPS 提交；
4. Server 校验 Machine ID/lifecycle/rate/schema；
5. Server 签发 Device leaf/chain；
6. Client 校验 key match、chain、profile、validity；
7. 原子保存；
8. Server 和 Client 记录 correlation/audit。

检查 request/response、DB 和日志中没有 Gateway CSR/SPKI/leaf/chain。

## 4. mTLS control

1. 使用 Device certificate 建立 QUIC；
2. Server 验证 Device mapping/lifecycle；
3. 完成 exact wire hello；
4. 检查 current connection registry；
5. 接收 initial Observed；
6. 确认 anonymous peer 在 TLS 阶段失败且 decoder counter 不增加；
7. 确认 0-RTT 关闭。

Enrollment 成功只表示 Device Identity 就绪，不表示 Gateway certificate 或 Caddy READY。

## 5. 失败处理

| 失败 | 行为 |
|---|---|
| 错 IP/CA/expiry | 修复 endpoint/trust/certificate；禁止 TOFU |
| Machine ID conflict | 停止，人工调查 lifecycle |
| CSR/profile invalid | 修正 Client/PKI；不使用通用签发 |
| local persist 中断 | 根据 journal/staging 恢复同一 request |
| duplicate request | 查询既有 Device/leaf；不得创建第二 lifecycle |
| mTLS rejected | 检查 leaf/profile/lifecycle/time；不降级 |
| Server/Client identity 不一致 | 停止并保存证据 |

## 6. 成功判定

- 唯一 active Device；
- Device key/leaf 匹配；
- Enrollment 只有 Device material；
- authenticated QUIC online；
- initial Observed；
- anonymous/0-RTT 负向证据；
- Caddy 仍可为 BLOCKED，Gateway 维度独立；
- audit 完整。

## 7. Evidence

```text
DEVICE_PK=
MACHINE_SHORT_ID=
ENROLLMENT_REQUEST_ID=
DEVICE_CERT_SERIAL=
DEVICE_SPKI=
CHAIN_SHA256=
QUIC_CONNECTION_ID=
WIRE_VERSION=
OBSERVED_SEQUENCE=
ANONYMOUS_TEST=
ZERO_RTT_TEST=
AUDIT_EVENT=
OWNER=
REVIEWER=
```
