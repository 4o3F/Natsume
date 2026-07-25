# Server Control Certificate Provisioning

> 适用：首次部署、到期轮换或批准的 Server endpoint 变化  
> 关键不变量：`INV-CERT-01`、`INV-SECRET-01`

## 1. 前提

- Server IP literal/port 已冻结；
- control root 的 custody、quorum 和离线介质已批准；
- 目标 Server 主机身份已核验；
- 时间同步正常；
- 当前 certificate inventory 已备份；
- 新 private key 将在 Server 受限边界生成或按批准 ceremony 导入；
- 有回滚到当前 leaf/chain 的路径。

运行中的 Server 不持有 offline root private key。

## 2. 生成请求

在受控 Server 边界生成新的 Server key 和 CSR。CSR 至少检查：

- key algorithm/size；
- Subject；
- IP-SAN 精确匹配冻结 endpoint；
- 不含未授权 DNS/SAN；
- profile/key usage；
- request fingerprint。

记录 CSR hash，不把 private key 复制到工单或普通共享目录。

## 3. 离线签发

1. 在离线签发环境导入 CSR；
2. 重新核对 endpoint、profile、validity 和 serial；
3. 使用批准的 control issuer 签发；
4. 导出 leaf/chain，不导出 root private key；
5. 保存 ceremony 记录、public certificate、fingerprint、serial 和 expiry；
6. 双人复核。

## 4. 安装

1. 备份当前 leaf/chain 和配置引用；
2. 将新 leaf/chain 写入 package-defined owner-only path；
3. 确认 private key/leaf SPKI 匹配；
4. 验证 chain、IP-SAN、EKU、not-before/not-after；
5. 运行 Server config validation；
6. reload/restart Server；
7. 从目标 Client 网络执行正确 trust/IP 连接；
8. 执行错误 IP/CA 负向检查；
9. 检查 Enrollment HTTPS 与 operator HTTPS 的实际 profile；
10. 记录 audit/change。

不要在 postinstall 中执行该 ceremony。

## 5. 成功判定

- Server 使用新 serial；
- 正确 IP/trust 成功；
- 错误 IP/trust 失败；
- Device mTLS/control 不被错误地改成 server-auth-only；
- Enrollment 仍只签 Device Identity；
- 日志无 private key/CSR body；
- 旧 leaf 按 retention policy 安全保留或销毁；
- certificate inventory 更新。

## 6. 回滚

当 config、chain 或 Client compatibility 失败：

1. 停止进一步 Enrollment；
2. 恢复已验证旧 leaf/chain；
3. reload/restart；
4. 重做正确/错误连接验证；
5. 标记新 serial 未使用/撤销；
6. 保存失败 evidence；
7. 不通过关闭 verification 或 TOFU 绕过。

## 7. Evidence

```text
SERVER_ID=
ENDPOINT=
OLD_SERIAL=
NEW_SERIAL=
CSR_SHA256=
LEAF_SHA256=
CHAIN_SHA256=
VALIDITY=
CEREMONY_RECORD=
POSITIVE_TEST=
NEGATIVE_TEST=
AUDIT_EVENT=
OWNER=
REVIEWER=
```
