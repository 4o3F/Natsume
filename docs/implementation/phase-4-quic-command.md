# Natsume V2 Phase 4 详细实施计划：QUIC mTLS、Observed 与可靠 Command

> 架构基线：`Natsume_V2_Design_v2.7.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.4.md`  
> 计划版本：Phase Plan v1.1  
> 基准窗口：W14–W22  
> Gate：G3  
> 前置依赖：Phase 3 Device certificate/mTLS；Phase 1 Operation/Command schema

---

## 1. 阶段使命与边界

建立经过强认证、资源有界、可恢复的长期 Device control session，并证明至少一次投递不会产生重复效果。Phase 4 冻结 Gateway certificate QUIC 子协议的消息、持久化与授权边界，但不要求 Caddy/真实 Gateway signer executor 全部完成；真实签发和 materialization 在 Phase 5 验收。

---

## 2. 详细工作包

### P4.1 Protocol finalization

`ControlEnvelope` 闭集：

- ClientHello/ServerHello；
- Heartbeat；
- ObservedStateSnapshot；
- Command/CommandStatus；
- BindingRequest/BindingResult；
- GatewayCertificateRequest/GatewayCertificateResult；
- ServerDrain/ProtocolError。

删除/禁止：

- DesiredStateStatus；
- generic CertificateIssueRequest/Result；
- INSTALL_CERTIFICATE Command；
- installation instance/token/clone reason；
- arbitrary path/shell/unit/upstream。

冻结 max frame、semantic limits、exact wire version、descriptor/golden fixtures。

### P4.2 Quinn/rustls mandatory mTLS

- Enrollment HTTPS 与 QUIC 使用完全独立 rustls config；
- TLS 1.3、ALPN `natsume-device/2`；
- 0-RTT disabled；
- Server mandatory client cert verifier；
- `peer_identity` extraction；
- SAN Machine ID、serial、fingerprint、Device state、ClientHello cross-check；
- handshake semaphore、idle timeout、keepalive。

### P4.3 Connection registry

- connection epoch；
- duplicate connection replacement/drain；
- old epoch terminal result acceptance；
- heartbeat/jitter/degraded/offline；
- bounded send queue and priorities；
- backpressure/slow consumer；
- source IP observation only。

### P4.4 Observed State

- observed sequence；
- received/applied generation/hash；
- apply status including `waiting_for_gateway_certificate`；
- installed credential revision；
- Gateway certificate fingerprint/not-after/state；
- Session/Home compact state；
- coalesced DB checkpoint；
- Drift API/UI。

### P4.5 Operation/Target/Command/Attempt

- frozen target selection/payload；
- deadline/offline policy；
- actor/reason/audit/idempotency；
- dispatcher priority/global/per-device semaphore；
- cancellation/superseded state；
- secret command ciphertext at rest；
- Operation aggregation。

### P4.6 Client command journal

- command payload persist + fsync before `RECEIVED`；
- step journal/terminal result/cursor；
- duplicate result replay；
- encrypted secret payload；
- crash at receive/running/result/send；
- retention/GC；
- resource lanes。

### P4.7 Gateway certificate request authorization contract

Request fields：request ID、command ID、generation、configuration revision、CSR DER、SPKI、nonce。

Server checks：

1. mTLS/Hello 已完成；
2. authenticated Device 与 command owner 相同；
3. command kind=`SYNC_STATE`，非 terminal，deadline 有效；
4. generation/config revision 与冻结 payload 相同；
5. CSR signature/key/SPKI valid；
6. request ID/command/SPKI idempotency；
7. different SPKI conflict；
8. SAN/profile 只从 command snapshot 派生。

本阶段可以使用 test signer/fixture certificate 返回，但必须持久化 request/result 并验证 retry 语义。

### P4.8 Typed dummy executors

- dummy SYNC_STATE/SYNC_SECRET/Session/Home；
- configuration/secret/session/home/diagnostics lane；
- Gateway request wait/resume dummy step；
- deadline/offline/cancel；
- no arbitrary shell/path/unit。

### P4.9 Fleet simulator

- 2,000 mTLS sessions；
- 200 active commands；
- reconnect storm；
- duplicate/reorder/delay/drop；
- slow consumer/queue pressure；
- Server/Device crash；
- cert reject/revoke/disabled；
- Gateway request idempotency/conflict/issuer delay simulation。

---

## 3. 实施顺序

### W14–W16：Transport/Handshake

- rustls configs；
- QUIC listener/client；
- Hello/registry/heartbeat；
- auth negative tests。

### W17–W18：Observed 与 persistence

- Observed schema/client reporter；
- coalesced checkpoint；
- Drift integration；
- connection replacement。

### W19–W20：Command reliability

- Operation/dispatcher；
- local journal；
- dummy executors；
- kill-point tests。

### W21：Gateway request contract

- request/result messages；
- command authorization；
- idempotent persistence；
- test signer；
- negative cases。

### W22：Scale/Gate

- 2,000/200 baseline；
- reconnect storm；
- protocol/security review；
- G3 evidence。

---

## 4. 交付物

- production Quinn/rustls control transport；
- final Protobuf descriptor/golden fixtures；
- connection registry；
- Observed/Drift pipeline；
- Operation/Command dispatcher；
- Client command journal；
- Gateway request authorization/persistence service；
- fleet simulator；
- G3 evidence bundle。

---

## 5. 验证矩阵

| 场景 | 预期 |
|---|---|
| anonymous/wrong CA/profile/SAN/revoked cert | handshake/application admission 前拒绝 |
| wire/ALPN mismatch | incompatible，禁止 Command/secret |
| 0-RTT attempt | disabled/rejected |
| duplicate command | stored terminal result，无第二次效果 |
| crash before journal fsync | 不得发送 RECEIVED |
| old connection Observed | 不覆盖 current epoch |
| reconnect | 只重发 outstanding Commands，不自动推 Target |
| Gateway CSR without command | reject |
| command belongs another Device | reject/security audit |
| wrong generation/config revision | reject |
| same request/SPKI | same result |
| same request different SPKI | conflict |
| CSR SAN malicious | ignored，fixture cert uses target SAN |
| 2,000 reconnect storm | bounded memory/FD/queue |

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| QUIC auth 配成 optional | dedicated config + anonymous handshake release blocker |
| command ACK 早于 durable journal | kill-point test + API invariant |
| Gateway request 变为通用证书 API | 专用消息、active command authorization、无 HTTP route |
| Simulator 与真实行为偏差 | physical/VM clients 参与每周 integration |
| Observed 高频写压垮 SQLite | memory registry + change/coalesced checkpoint |
| 老 connection 覆盖新状态 | connection epoch ownership rules |

---

## 7. G3 Gate 清单

- [ ] real Device certificates + mandatory mTLS end-to-end；
- [ ] anonymous/wrong cert 在 parser 前拒绝；
- [ ] exact wire/ALPN/0-RTT 规则通过；
- [ ] 2,000 connections baseline 稳定；
- [ ] Command fsync-before-RECEIVED；
- [ ] duplicate command effectively-once；
- [ ] bounded queues/reconnect jitter；
- [ ] Observed 是唯一 apply status source；
- [ ] reconnect 不自动推 Target；
- [ ] Gateway request active-command authorization/idempotency/conflict 通过；
- [ ] 无 generic certificate issue/install command；
- [ ] G3 evidence 已签署。
