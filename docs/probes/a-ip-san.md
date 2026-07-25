# Probe A — IP-SAN 与 endpoint

> Status: `NOT-RUN`  
> Requirements: `REQ-P0-041`、`REQ-P0-042`、`REQ-P0-050`、`REQ-P0-051`  
> Gates: `G0-002`、`G0-003`

## 目标

证明 Client 安装/升级保留明确 Server IP literal 与 port，TLS 只接受正确 IP-SAN/trust，并验证同一数字端口上的 TCP HTTPS 与 UDP QUIC 在目标网络可工作。

## 环境

```text
COMMIT_SHA=
SERVER_OS=
CLIENT_OS=
SERVER_IP=
PORT=
CERT_SERIAL=
FIREWALL=
OWNER=
REVIEWER=
DATE=
```

不得使用生产 private key 或未脱敏现场地址作为公开 artifact。

## 步骤

1. clean install Client，以交互方式配置 endpoint；
2. 检查持久化值、owner/mode 和 daemon 校验；
3. reinstall/upgrade，确认值保持；
4. explicit reconfigure，确认只有明确操作会修改；
5. 使用正确 CA + IP-SAN 连接 Enrollment HTTPS；
6. 分别测试错误 IP、错误 CA、过期/not-yet-valid certificate；
7. 在同一数字 port 启动 TCP HTTPS 与 UDP QUIC；
8. 从目标 Client 网络验证二者；
9. 检查无 TOFU/dangerous verifier；
10. 保存命令、证书 inspection、packet/firewall evidence。

## 预期

- 正确配置成功；
- 四类错误明确失败且有稳定码；
- upgrade 不改 endpoint；
- TCP/UDP 可共存；
- 无 DNS/TOFU fallback。

## 结果

```text
STATUS=NOT-RUN
POSITIVE_RESULTS=
NEGATIVE_RESULTS=
ARTIFACTS=
LIMITATIONS=
```
