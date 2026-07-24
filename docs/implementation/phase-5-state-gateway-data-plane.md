# Natsume V2 Phase 5 详细实施计划：显式 State/Secret、Gateway 证书与离线数据面

> 架构基线：`Natsume_V2_Design_v2.7.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.4.md`  
> 计划版本：Phase Plan v1.1  
> 基准窗口：W20–W29  
> Gate：G4  
> 前置依赖：G2A、G2B、G3；Origin Issuing Intermediate 可用

---

## 1. 阶段使命与核心闭环

形成第一条真实、可断电恢复、不会自动分发密码的数据链路：

```text
CSV/Binding/Configuration
→ explicit SYNC_STATE
→ Gateway key/CSR lazy generation
→ authenticated QUIC Gateway certificate issuance
→ visual BLOCKED local HTTPS
→ human-only SYNC_SECRET
→ encrypted LKG
→ Caddy READY
→ DOMjudge auto-login
→ Server offline + Client reboot restore
```

本阶段是 Gateway certificate 的首个生产实现。Enrollment 必须保持只签 Device certificate。

---

## 2. `SYNC_STATE` 与 Gateway certificate 的详细事务

### 2.1 Server 创建命令

- operator 或允许的 AutomationPolicy 选择目标；
- 冻结 Device、generation、canonical hash、configuration revision；
- 计算每台 Device 的 certificate action：`reuse | issue | reissue`；
- `GatewayCertificateMode`：`ENSURE_VALID` 或受审计的 `FORCE_REISSUE`；
- 创建 Operation/Target/Command/Audit/Change 同事务；
- Command payload 无 password、无 certificate private material。

### 2.2 Device 接收与本地持久化

1. 写 command journal + fsync；
2. 回报 RECEIVED；
3. 验证 generation/hash/schema/deadline/capability；
4. 写 state-apply transition journal；
5. reassignment 时先 visual BLOCKED/停止 Browser/清旧 secret/LKG；
6. 应用非秘密 assignment/config；
7. 进入 `ENSURING_GATEWAY_CERTIFICATE`。

### 2.3 Gateway credential 判定

已有 encrypted Gateway key/cert 同时满足：

- certificate chain to Local Origin Root；
- SPKI 与 private key 匹配；
- SAN 等于 target `client_origin_hostname`；
- profile ID、EKU serverAuth、KeyUsage、CA=false；
- not-after ≥ target minimum validity；
- 未 revoked/corrupt；
- mode 不是 FORCE_REISSUE。

满足则复用；否则生成新的 Gateway private key。

### 2.4 Client request journal

在发送 CSR 前持久化：

```text
gateway_certificate_request_id
command_id
target_generation
configuration_revision_id
spki_sha256
csr_der or encrypted request material
request_nonce
state=pending
```

Private key 先作为 encrypted vault record 写入并 fsync。相同 command 重启后复用同一 request ID/CSR/SPKI，不重复生成 key。

### 2.5 QUIC request/Server authorization

Daemon 通过当前 mTLS control stream 发送 `GatewayCertificateRequest`。Server：

1. 从 peer certificate/connection registry 得到 Device；
2. 读取 command，验证 owner/kind/state/deadline；
3. 对比 generation/configuration；
4. 验证 CSR signature/key/SPKI；
5. 查询 idempotent request；
6. 从 frozen TargetGateway 派生 SAN/profile/minimum validity；
7. 用 Origin Issuing Intermediate 签发；
8. 独立 parser 验证；
9. 在事务中写 gateway certificate、request issued、Audit/Change；
10. 返回 `GatewayCertificateResult`。

Server 不读取或采信 CSR 中的 SAN、EKU、CA flag。没有 active command 时绝不签发。

### 2.6 Device 验证、落盘与 Caddy materialization

- 验证 chain/root、SPKI、SAN、EKU、KeyUsage、CA=false、serial、validity；
- encrypted vault transaction 写 Gateway key/cert chain；
- request journal terminal result；
- decrypt 到 `/run/natsume/gateway-tls/<generation>/`；
- mode/ownership、atomic current switch、ready marker；
- 启动/更新 Caddy visual BLOCKED；
- local HTTPS probe；
- commit target applied generation/hash。

没有 secret 时状态是 `APPLIED + blocked_secret_missing`，不是 READY。

### 2.7 错误与恢复

稳定错误码至少：

```text
GATEWAY_CERT_REQUEST_NOT_AUTHORIZED
GATEWAY_CERT_COMMAND_MISMATCH
GATEWAY_CERT_REQUEST_EXPIRED
GATEWAY_CERT_CSR_INVALID
GATEWAY_CERT_SPKI_CONFLICT
GATEWAY_CERT_ISSUER_UNAVAILABLE
GATEWAY_CERT_PROFILE_INVALID
GATEWAY_CERT_LOCAL_KEY_MISMATCH
GATEWAY_CERT_INSTALL_FAILED
```

断线/Server restart：Client 用相同 request ID/SPKI 重试，Server 返回同一 issued certificate。Issuer 暂不可用：Command 保持 waiting/backoff；超过 deadline 失败并保持 Caddy absent/BLOCKED。禁止降级到 Enrollment、匿名 HTTPS、自签证书或跳过校验。

---

## 3. 其他详细工作包

### P5.1 Canonical Target 与 SYNC_STATE executor

- TargetGateway 包含 configuration revision、origin hostname、upstream/login profile、certificate profile/minimum validity；
- no expected fingerprint；
- assignment/config apply journal；
- superseded command handling；
- clear-old-secret-first；
- Observed progress/error。

### P5.2 Human-only SYNC_SECRET

- Web preview、re-auth、reason、frozen targets；
- Server vault ciphertext until dispatch；
- Client encrypted command record before RECEIVED；
- assignment/credential/deadline checks；
- secret type zeroize/no logs；
- duplicate/recovery；
- Automation cannot create；
- bulk Operation summary。

### P5.3 Encrypted LKG

- record schema/AAD；
- applied state/assignment/credential；
- Gateway certificate fingerprint；
- fixed upstream/login matcher；
- secret；
- runtime config inputs/hash；
- activation pointer/journal；
- GC/clear/reset；
- no standalone plaintext/bin secret。

### P5.4 Visual status page

状态：

```text
restoring
transition_blocked
configuration_missing
certificate_requesting
certificate_invalid
secret_missing
upstream_unhealthy
recovery_required
unassigned
```

- package-local assets；
- HTML 503；
- CSP/no-store/no remote resource；
- enum-only JSON；
- `textContent`；
- no `session_locked`；
- injection tests。

### P5.5 Caddy Admin/activation

- Reqwest Unix socket；
- fixed Native JSON generator；
- visual bootstrap；
- runtime load/health；
- Caddy process epoch/restart detection；
- fixed upstream/exact login matcher/header stripping；
- no secret access logs；
- activation journal。

### P5.6 DOMjudge integration

- local HTTPS/HTTP2；
- login X-Headers；
- Cookie/CSRF/redirect/logout；
- submission/clarification/scoreboard；
- Brotli transparent；
- upload/long response/timeouts；
- upstream failure；
- optional direct-access restriction。

### P5.7 Fault/recovery matrix

- kill/reboot at every SYNC_STATE/Gateway request step；
- kill after key persisted/before request；
- request issued/server committed/result lost；
- cert received/before vault commit；
- vault commit/before `/run` materialize；
- materialize/before Caddy ready；
- state applied/secret absent；
- secret stored/Caddy load failure；
- Caddy/Daemon restart；
- Server offline steady reboot；
- identity/vault/cert invalid；
- password revision/stale secret。

---

## 4. 实施顺序

### W20–W22

- Target/SYNC_STATE executor；
- Gateway credential requirement evaluator；
- request journal/client key generation；
- Server authorization/signing service。

### W23–W24

- result validation/vault storage；
- `/run` materialization；
- visual Caddy bootstrap/status；
- first end-to-end Gateway certificate via QUIC。

### W25–W26

- SYNC_SECRET；
- LKG/activation journal；
- Caddy runtime config；
- DOMjudge login contract。

### W27–W28

- reboot/crash/offline matrix；
- Caddy/Daemon restart；
- bulk operations；
- security/canary tests。

### W29

- complete gate scenario；
- operator UAT；
- G4 evidence。

---

## 5. 交付物

- production SYNC_STATE/SYNC_SECRET executors；
- Gateway request client/server/signing/persistence；
- Gateway certificate profile validator；
- Client Gateway key/cert vault lifecycle；
- Caddy visual assets/Admin adapter/activation journal；
- encrypted LKG；
- DOMjudge integration suite；
- fault matrix automation；
- state/secret/gateway recovery runbooks；
- G4 evidence bundle。

---

## 6. 验证矩阵

| 场景 | 预期 |
|---|---|
| fresh enrolled Device, no Gateway key | SYNC_STATE lazily generates key/CSR and obtains cert over QUIC |
| Enrollment API used for Gateway CSR | route/schema absent |
| no mTLS / wrong Device | request rejected |
| no active command / wrong generation/config | request rejected |
| malicious CSR SAN | ignored; cert uses target hostname |
| same request/SPKI after result loss | same certificate returned |
| same request different SPKI | conflict/security audit |
| Origin issuer unavailable | waiting/fail by deadline; no self-signed fallback |
| crash after key persisted | resume same request/key |
| crash after Server issued but before Client received | idempotent result recovery |
| cert profile/SPKI mismatch | reject local install, Caddy blocked |
| state applied, no secret | visual HTTPS works, proxy blocked |
| reassignment | old secret cleared before new state commit |
| duplicate SYNC_SECRET | no second secret effect |
| steady reboot without Server | Caddy READY from vault/LKG |
| transition reboot | old account never restored |
| Session lock | zero Caddy configuration change |

---

## 7. 风险与缓解

| 风险 | 缓解 |
|---|---|
| Gateway signing 与 Command transaction 脱节 | request table + command FK/context + same-result idempotency |
| CSR/private key 在日志/WAL 泄漏 | encrypted records、redacted types、canary scan；CSR可存但不记录正文 |
| 首次 SYNC_STATE 依赖 issuer 导致操作延迟 | readiness 预检、issuer health、bounded concurrency、明确 waiting UI |
| 证书签发风暴 | per-device one-live-request、global signer semaphore、bulk pacing |
| 状态 APPLIED 被误解为 READY | UI 分开 state apply、secret state、Gateway state |
| Server 离线时证书首次请求无法完成 | fail closed；已有 valid cert/LKG 才允许离线恢复 |
| FORCE_REISSUE 造成不必要中断 | re-auth/preview/reason，默认 ENSURE_VALID |

---

## 8. G4 Gate 清单

- [ ] Enrollment 仍只签 Device certificate；
- [ ] fresh Device 的首次 Gateway key/CSR/cert 由 SYNC_STATE + mTLS QUIC 完成；
- [ ] request 绑定 Device/command/generation/configuration/SPKI；
- [ ] Server 按 target 派生 SAN/profile，忽略 CSR SAN；
- [ ] request/result 幂等、断线/restart 可恢复；
- [ ] Target 无 secret/expected certificate fingerprint；
- [ ] SYNC_SECRET human-only/re-auth/reason/audit；
- [ ] persistent Gateway key/secret/LKG 均 ciphertext，plaintext only `/run`/memory；
- [ ] visual status 安全；
- [ ] DOMjudge contract 通过；
- [ ] steady offline reboot 通过；
- [ ] transition 不恢复旧 account；
- [ ] G4 evidence 已签署。
