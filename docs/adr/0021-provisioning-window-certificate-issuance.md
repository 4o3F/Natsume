# ADR-0021: Provisioning-window certificate issuance

> Status: `ACCEPTED`  
> Scope: Natsume V2  
> Supersedes: [ADR-0016](0016-gateway-certificate-issued-during-sync-state.md)  
> Superseded by: —

## Context

部署模型（现固化于 [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)）有三个决定证书设计的事实：

1. **目标规模约 500 台受管工作站**，单机房有线 LAN，单 Server 实例，单场竞赛生命周期。
2. **所有工作站安装同一份赛事专用 OS 镜像**，由本项目构建。
3. **赛事开启前存在一段物理安全窗口**：工作站处于受控场地、由操作员摆放和配置，不存在对抗性使用者。窗口结束、选手入场后，本机使用者不再受信。

由此产生一个受信 provisioning 窗口与一个不受信运行窗口的清晰分界。

本机 HTTPS 的保留依据（v2.8 补记）：浏览器只在加密 scheme 上通告 `Accept-Encoding: br`；v1 实测 brotli 相对 gzip 的带宽节约显著，而带宽是受限资源（ADR-0022 F2/F5）。因此 Gateway certificate 是必要机制。压缩发生在 DOMjudge web server，本机 Caddy 保持 `Accept-Encoding` 透传（见 [ADR-0024](0024-domjudge-autologin-via-xheaders.md)）。

Gateway certificate 供本机 Caddy 在 loopback 上服务 Managed Browser。其 hostname 是**每场地一个常量**，在镜像构建期未知（因此不能烤入镜像），但在安装期已知——与 Server endpoint 属于同一类部署参数，`packaging/client/debconf` 已有现成机制。local origin CA 已在包构建期通过 `${LOCAL_ORIGIN_CA_CERT}` 注入 `/etc/natsume/trust/`。

[ADR-0016](0016-gateway-certificate-issued-during-sync-state.md) 把 CSR 绑定到 active `SYNC_STATE` Command。在上述部署模型下其前提（Enrollment 时授权条件不存在或会变化）不成立：binding 在 provisioning 窗口内建立，hostname 是场地常量。其结果是一套子协议在派生一个全场恒定的值。

## Decision

**一切签发只发生在 provisioning 窗口内，且只在 Enrollment 路径上。**

1. Server 维护显式且持久化的 provisioning 窗口状态。默认关闭；开启与关闭都是受审计的操作员动作；故障恢复、重启与备份还原后不得自动开启。
2. 窗口开启时，Enrollment 在**同一事务**中为每台 Device 签发两件产物：**Device Token**（控制面认证，机制见 [ADR-0023](0023-wss-control-channel-with-device-token.md)）与 **Gateway certificate**（本地数据面 server auth）。证书 profile 只有 Gateway 一种；不存在通用 certificate endpoint。
3. **窗口关闭后 Server 拒绝一切签发**（token 与证书同规则）。设备替换或重签是显式重开窗口的受审计例外，不是常态开放路径。
4. Gateway hostname 是**每场地一个常量**，在安装期经 debconf 提供并记入 client 配置，与 Server endpoint 使用同一机制与同一 canonical 校验。**不从 Target 派生。**
5. Gateway key 在 Client 本地生成，**私钥不离开设备**。Server 从站点配置与冻结 policy 派生 SAN、profile 与 validity；**CSR 自报的 SAN/CN/profile 不授予任何权限**，只用于证明 possession 与公钥结构。
6. 同一 `MachineHardwareId` 在同一窗口内重复 Enrollment 为**受审计的替换**：旧 token 立即失效，签发新 token 与新 leaf。不维护独立的 SPKI 冲突状态机。
7. Gateway certificate validity 必须覆盖 provisioning 窗口起点至赛事结束加明确余量；该值在站点配置中显式选择，不使用隐式默认，且有赛前校验。
8. **不建吊销分发机制**：浏览器对私有 CA 不查 CRL/OCSP；DB 中的 revoked/retired 仅作运维台账。每证书跟踪字段收敛为 serial、SPKI fingerprint、not-after、status。

证书阶梯为两段：

```text
server-auth TLS（全部入口，预置 trust + IP-SAN 验证）
  → provisioning 窗口内 Enrollment：签发 { Device Token + Gateway certificate }
  → WSS 控制面（token 认证）；SYNC_STATE / SYNC_SECRET 不签发任何东西
```

**据此移除**：CSR 嵌入 active `SYNC_STATE` Command 的子协议；CSR 与 `command_id`、configuration generation、assignment revision 的绑定；SPKI 冲突检测状态机；Target 派生 SAN；Device Identity certificate（由 Device Token 替代，见 ADR-0023）。

## Alternatives

- **维持 ADR-0016**：其 Target 派生在每场地常量 hostname 下无内容；代价是控制协议层嵌套子协议、跨 Phase 实现纠缠，以及 500 台首次上线时 500 次额外多轮往返。
- **Gateway leaf 烤入镜像**：hostname 在镜像构建期未知，不可行。
- **全场共用一份 Gateway key/leaf 并烤入镜像**：500 台共享私钥，且仍受 hostname 未知约束。
- **设备自签 + 首次开机写入浏览器信任库**：需在每台上操作 Firefox/Chromium 信任库；而 local origin CA 已在包构建期注入，该方案用更难的问题替换更易的问题。
- **运行期保持 Enrollment 开放**：选手入场后本机使用者不受信，被攻陷设备可在赛事期获取新凭据。
- **改用 `http://localhost` 取消 Gateway certificate**：浏览器在非加密 scheme 上不通告 brotli，将放弃实测显著的带宽节约（ADR-0022 F5）；不采用。

## Consequences

### Positive

- 签发面在时间上封闭：不受信窗口内不存在任何签发路径；
- 私钥不离机与 Server 决定授权属性两项性质完全保留；
- Gateway 不进入 Command runtime，状态同步与证书解耦；
- 500 台批量上线只需一次 Enrollment 往返；
- 阶梯短（两段）且可用负向测试穷举；
- 误克隆磁盘在替换语义下当场显形（旧机 401 掉线 + 异常审计）。

### Negative / trade-offs

- provisioning 窗口成为新的 Server 持久状态，必须默认关闭、可审计、且在故障恢复后不得自动开启；
- Gateway certificate 与 Device Token 不重开窗口即无法轮换；单场竞赛生命周期内可接受，跨赛事复用必须走 single-lifetime reset；
- validity 选择错误会在赛事中途失效，且此时无签发路径——必须有赛前校验。

## Evidence and revisit trigger

需要的负向证据：

- 窗口关闭时任何签发请求被拒绝，且不改变 Server truth；
- Enrollment 之外不存在签发路径（schema/路由/DB 断言）；
- CSR 自报 SAN/CN/profile 被忽略，签发结果只来自站点配置；
- 同窗口内重复 Enrollment 的替换语义与审计记录；
- 故障恢复、重启与备份还原后窗口不自动开启；
- Gateway validity 覆盖赛事全程的赛前校验。

重新打开该决策的条件：出现每设备不同 hostname 的需求；出现多场地或多生命周期部署；或物理安全窗口假设不再成立。任何重开都必须 supersede 本 ADR，不得在现有路径上加特例。

## References

- [ADR-0016](0016-gateway-certificate-issued-during-sync-state.md)（本 ADR 替代）
- [ADR-0022](0022-deployment-facts-and-trust-assumptions.md)
- [ADR-0023](0023-wss-control-channel-with-device-token.md)
- [ADR-0024](0024-domjudge-autologin-via-xheaders.md)
- [security-recovery.md](../security-recovery.md)
- [contracts.md](../contracts.md)
- [architecture.md](../architecture.md)
