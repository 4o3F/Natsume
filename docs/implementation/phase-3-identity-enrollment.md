# Natsume V2 Phase 3 详细实施计划：Machine ID、Client Vault 与 Device-only Enrollment

> 架构基线：`Natsume_V2_Design_v2.5.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.2.md`  
> 计划版本：Phase Plan v1.0  
> 基准窗口：W7–W16  
> Gate：G2B  
> 前置依赖：G0；Phase 1 的 Enrollment/PKI/Vault schema 可并行稳定

---

## 1. 阶段使命与硬边界

在任何控制命令、DOMjudge secret、LKG 或 Caddy runtime material 被使用前，完成本地硬件身份判断、Client vault 打开和 Daemon 控制证书取得。

Phase 3 的最终输出只有：

```text
MachineHardwareId
+ independent identity file
+ Client encrypted vault
+ Device Identity private key/certificate
+ mandatory-mTLS QUIC 可连接
```

Phase 3 **不生成 Gateway private key，不提交 Gateway CSR，不签发 Gateway certificate，不启动 Caddy READY**。Gateway credential 的首次生成与签发属于 Phase 5 的 `SYNC_STATE`。

---

## 2. 详细工作包

### P3.1 Fixture-first Machine Identity

- normalization、placeholder catalog、字符/长度规则；
- 6+ 物理硬件 fixture；
- VM、SATA/NVMe、disk replacement、firmware missing、permission denied；
- candidate quality/priority；
- immutable `fleet_namespace_uuid` 下 deterministic UUIDv5；
- raw serial 只在 Privileged Helper 内存短暂存在。

### P3.2 Pure `machine-identity` crate

- component fingerprint/candidate derivation；
- fixed priority selection；
- startup decision：`matched | indeterminate | mismatch`；
- local preflight：clean first start、record missing/corrupt、site namespace mismatch；
- no alias graph、version、installation instance、Linux I/O；
- property/fuzz/golden tests。

### P3.3 Privileged Helper collector

- sysinfo Product/Motherboard；
- smbios-lib supplement；
- raw-cpuid actual processor serial only；
- procfs MountInfo + udev root disk；
- typed D-Bus response；
- bounded timeout/error classification；
- PrivateNetwork、caller policy；
- 无 CLI/text fallback。

### P3.4 Independent identity file

路径 `/var/lib/natsume/identity/machine-hardware-id`：

- schema version、site namespace、Machine ID、checksum；
- O_NOFOLLOW、temp、fsync、rename、parent fsync；
- owner/mode；
- package/reinstall/upgrade preservation；
- raw serial 不落盘。

判定：

- 所有 identity-bound artifact 均不存在才是 clean first start；
- identity file 缺失/损坏但 DB/key/cert/LKG 存在 → fail closed；
- site namespace mismatch → fail closed；
- evidence temporarily unavailable → 不删除、重试；
- conclusive mismatch → local reset。

### P3.5 Client root key 与 encrypted DB

- 32-byte CSPRNG root key、O_EXCL/0400/fsync；
- HKDF salt=Machine ID；
- SQLite schema/key-check；
- record-level AEAD/AAD/key version；
- record types：Device private key/cert chain、future Gateway key/cert、secret、LKG、pending command；
- Gateway record type 可以定义，但首次启动不得创建记录；
- wrong key/tamper/corruption/WAL/temp canary tests。

### P3.6 Daemon-integrated startup check

严格顺序：

```text
read root-owned site/endpoint config
→ inventory identity-bound artifacts
→ validate identity record/site namespace
→ collect current candidates
→ classify match/indeterminate/mismatch
→ matched 时 derive/open vault
→ vault key-check/integrity
→ load or create Device Identity key
→ Enrollment or QUIC
```

Mismatch reset：停止 Caddy、删除 Client DB/root key/Device/Gateway cert/LKG/journal/identity file，fsync 后回到普通 first start；不记录 clone reason，不自动吊销源设备。

### P3.7 Endpoint 与 trust

- `/etc/natsume/config.toml` 的 canonical IP/port；
- TCP HTTPS 与 UDP QUIC 同数字端口；
- package-installed Control Root；
- package-installed Local Origin Root 只供未来 Gateway chain 验证；
- rustls IP ServerName/SAN；
- no TOFU/dangerous verifier；
- endpoint 只允许本地 administrator 改。

### P3.8 Device-only Enrollment HTTPS

Request：

- Machine Hardware ID；
- evidence quality/claim；
- **Device Identity CSR DER/SPKI**；
- software version；
- challenge/request nonce/signature。

Response：

- pending request ID/poll challenge；
- approval 后只返回 Device clientAuth leaf/chain；
- 无 Gateway CSR/SPKI/certificate 字段。

流程：challenge → signed request → pending → manual/auto approval → Device-key signed poll → result。Rate/body/TTL/idempotency/conflict 完整。

### P3.9 Device PKI 与 Server control PKI

- offline Control Root 签 Server IP-SAN leaf；
- per-instance Device Issuing CA 签 clientAuth leaf；
- Local Origin Root/Origin Intermediate 可以完成 Server 初始化与健康检查，但本阶段不签任何 Gateway leaf；
- Device leaf SAN=`urn:natsume:device:<machine_id>`；
- post-sign parser 验证 profile/serial/validity/SPKI；
- Server issuing keys/Client key 均 encrypted vault；
- revoke/rekey Device certificate workflow。

### P3.10 Device lifecycle

- pending/enrolled/revoked/disabled；
- same Machine ID + same Device SPKI idempotent；
- same ID + different Device SPKI conflict；
- confirmed rekey existing Device requires operator revoke/approve；
- unbind/revoke/delete ordering；
- no merge/split；
- identity reset/cert loss 走普通 Enrollment。

---

## 3. 实施顺序

### W7–W9：Identity core 与 fixtures

- physical fixture capture；
- pure crate；
- Helper collector；
- startup decision tests。

### W10–W12：Identity file 与 Client vault

- atomic identity record；
- root key/DB/key-check；
- integrated startup；
- mismatch/indeterminate/corruption fault matrix。

### W13–W14：Endpoint/trust 与 Enrollment

- IP-SAN client；
- challenge/request/poll；
- manual/auto approval；
- Device-only schema。

### W15：PKI 与 mTLS bootstrap

- Device leaf profile；
- encrypted install；
- first QUIC handshake with Device cert；
- Gateway credential absence assertions。

### W16：Physical/copy tests 与 Gate

- configured disk copy；
- key loss/corruption；
- rekey/conflict；
- G2B evidence。

---

## 4. 交付物

- Machine Identity crate、collector、fixtures；
- independent identity record implementation；
- Client vault/schema/root key；
- Daemon startup state machine；
- endpoint/trust config；
- Enrollment HTTPS service/client；
- Device PKI profiles；
- Device lifecycle UI/API；
- identity/vault/enrollment runbooks；
- G2B evidence bundle。

---

## 5. 验证矩阵

| 场景 | 预期 |
|---|---|
| fresh install | ID → identity file → root key/vault → Device key → pending Enrollment |
| image copied before first start | 每台硬件产生自身 ID/root key/Device key |
| configured disk copied | vault 前 mismatch，清理，普通 first start |
| collector temporary failure | 不删数据、不打开 vault/Caddy，重试 |
| identity file missing but DB/key exists | fail closed |
| site namespace mismatch | fail closed，不生成 replacement ID |
| identity match + DB auth fail | `VAULT_CORRUPT`，不新建 Device |
| same ID + same Device SPKI | idempotent request/result |
| same ID + different Device SPKI | conflict，无 auto approval |
| wrong Server CA/IP | Enrollment rejected |
| Enrollment request carries Gateway CSR | schema/contract rejected |
| Enrollment result carries Gateway leaf | test failure |
| after enrollment local vault | Device key/cert present；Gateway key/cert absent |
| no Device cert | QUIC control impossible |

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 同传后的瞬时 evidence 缺失导致误删 | 只有完整可信 mismatch 才 reset；indeterminate 永不删除 |
| root key 与 Machine ID 绑定误当秘密熵 | 随机 root key 提供熵，Machine ID 仅为 HKDF salt/AAD |
| Enrollment 又加入 Gateway CSR | proto/SQL/OpenAPI negative scan + Gate checklist |
| Auto approval 滥用 | default off、subnet/quality/limit/conflict/rate |
| Origin Intermediate 未就绪阻塞 Enrollment | control readiness 与 gateway issuer health 分离 |
| Device cert 过期后无法 QUIC rekey | 明确回到 Enrollment HTTPS 的 rekey workflow |

---

## 7. G2B Gate 清单

- [ ] identity-before-vault fault matrix 通过；
- [ ] configured-disk copy 无法使用旧 key/secret/cert；
- [ ] missing/corrupt identity record 不误走 first start；
- [ ] vault corruption 与 identity mismatch 分离；
- [ ] Client endpoint/trust/IP SAN 通过；
- [ ] Enrollment 只有 Device CSR 与 Device leaf/chain；
- [ ] manual/auto approval 与 conflict/rekey 通过；
- [ ] Device cert 可建立 mandatory-mTLS QUIC；
- [ ] Gateway key/cert 在 Phase 3 结束时保持 absent；
- [ ] 无 token/installation instance/clone service/merge-split；
- [ ] G2B evidence 已签署。
