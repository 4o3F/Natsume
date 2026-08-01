# ADR-0023: WSS control channel with Device Token authentication

> Status: `ACCEPTED`  
> Scope: Natsume V2 Device control plane  
> Supersedes: [ADR-0012](0012-server-auth-enrollment-and-mtls-control.md)  
> Superseded by: —

## Context

原架构将 Device control 冻结为 "QUIC + mandatory mTLS + Protobuf"，但该传输选择从未有过决策记录。按 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md) 的事实重新评估：

- **F2**（第三方管理网络、UDP 通过性无保证）使 QUIC 成为可用性风险而非收益：UDP 被中间设备限速或过滤即控制面整体瘫痪；QUIC 独有的连接迁移收益（应对 F3 的 DHCP 换 IP）在 durable Command journal + 幂等重投递面前只是一次秒级重连。
- 控制面需要**双向语义**（Server 推 Command，Device 推 Observed/receipt），轮询或单向 SSE 模拟双向会把超时、节奏、空轮询留给应用层。
- mTLS client certificate 的教科书优势在本部署无兑现场景：Device 私钥与任何 token 一样是同权限磁盘文件（T2 下失窃前提相同）；吊销在私有 CA 场景实际都靠 Server 端 DB 判断；且全系统只有一个验证方。
- Server 已为 operator API 与 Enrollment 运行 axum/rustls HTTPS 栈。

## Decision

### 传输

1. Device control 使用 **WebSocket over server-auth TLS（WSS）**；Protobuf 消息直接作为 WS binary frame（一帧一消息），**不再定义自定义 length-prefix framing**。
2. 协议版本经 `Sec-WebSocket-Protocol`（如 `natsume.v1`）协商；不匹配在 upgrade 阶段拒绝。
3. Frame 大小上限、封闭 envelope kind、未知 enum/版本显式失败等契约语义保留（见 [contracts.md](../contracts.md)）。
4. TLS early data（0-RTT）保持关闭（rustls 服务端默认）。

### 认证

5. Device 身份凭据为 **Device Token**：Server 端 32 字节 CSPRNG 生成的不透明随机值；DB 只存哈希（SHA-256/BLAKE3 级别即可——输入是 256-bit 随机值，不是人选口令）；WS upgrade 时经 `Authorization: Bearer` 提交，常数时间比对；**认证失败返回 401，发生在任何 Protobuf 解码之前**。
6. **不使用 JWT**：单验证方 + 本地 DB 使无状态验证无收益，而签名密钥保管、过期/刷新机制、吊销 denylist 全是净增机制。
7. **不设 token TTL**：时间边界由 Gateway certificate validity 承担，避免制造第二个"赛中失效且无签发路径"陷阱。失效仅三条显式路径：操作员吊销（删行，设备下次重连 401 → fail closed）；窗口重开 re-enrollment（替换语义，旧 token 自动作废）；single-lifetime reset。
8. Token 签发只发生在 provisioning 窗口内的 Enrollment 事务中，与 Gateway certificate 同一往返（流程见 [ADR-0021](0021-provisioning-window-certificate-issuance.md)）。同一 `MachineHardwareId` 窗口内重复 Enrollment 为**受审计的替换**：旧 token 立即失效、签发新 token 与新 leaf。副产品：误克隆磁盘的第二台机器注册时会立刻踢掉第一台（401 掉线 + 异常审计事件），事故当场可见。

### 拓扑

9. Operator API、Enrollment、Device WSS **合并到同一 TCP 端口**，各自独立路由、授权与限流；防火墙面收敛为一个 TCP 端口。
10. 继承自 ADR-0012 且继续有效：Enrollment 为 server-auth HTTPS，Client 验证预置 trust 与 IP-SAN，**无 TOFU、无 dangerous verifier**；未认证输入不进入协议 decoder。

## Alternatives

- **QUIC + mTLS（原冻结项）**：见 Context；独立协议栈 + 自定义 framing + UDP 风险，收益全部落空。
- **mTLS over TCP/WSS**：安全性与 token 等价（T2），但需要 Device Identity CA、CSR 路径、client-cert verifier 与 peer-cert 提取管道；被 token 严格减法替代。
- **HTTP 长轮询 / SSE + POST**：带宽并非瓶颈（keep-alive 下连接数与 QUIC 相同），但双向语义需应用层模拟，劣于原生全双工。
- **gRPC bidi streaming（tonic）**：引入第二个 HTTP/2 栈与代码生成链，重于 axum 原生 WS。

## Consequences

### Positive

- 删除 quinn 依赖、自定义 framing 契约整节及其负向测试面；
- 删除 Device Identity CA 与设备证书生命周期；
- 消息边界、keep-alive（ping/pong）、版本协商均由成熟协议承担；
- 单端口拓扑，防火墙验证项从"TCP/UDP 双协议"缩为"一个 TCP 端口"；
- 匿名拒绝语义可测试性更好（401-before-decode）。

### Negative / trade-offs

- token 为 bearer 凭据，每次连接在 TLS 内传输；依赖 server-auth TLS 验证纪律（预置 CA + IP-SAN）；
- 设备身份无密码学 possession 证明；跨互联网/多场地部署时不充分；
- TCP 队头阻塞理论存在，但消息小且低频，无实际影响。

## Evidence and revisit trigger

- 接受前需要的负向证据：无 token / 错误 token / 已吊销 token 的 upgrade 在解码前被拒；超限 frame 与未知 subprotocol 关闭连接；重连后 durable Command 收敛。
- 重开条件：出现跨互联网或多场地部署（T4 范围扩大）、或出现第三方需要验证设备身份——此时以新 ADR 引入 mTLS，认证层局部替换，不影响其余契约。

## References

- [ADR-0012](0012-server-auth-enrollment-and-mtls-control.md)（被本 ADR 替代）
- [ADR-0021](0021-provisioning-window-certificate-issuance.md)
- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [contracts.md](../contracts.md)
- [security-recovery.md](../security-recovery.md)
