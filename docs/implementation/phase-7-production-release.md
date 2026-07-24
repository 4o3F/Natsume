# Natsume V2 Phase 7 详细实施计划：Productionization 与正式发布

> 架构基线：`Natsume_V2_Design_v2.7.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.4.md`  
> 计划版本：Phase Plan v1.1  
> 基准窗口：W31–W44  
> Gate：G6 Release Candidate、G7 Production Ready  
> 前置依赖：G4；G5 在 RC freeze 前完成

---

## 1. 阶段使命

把功能完整的系统变成可安装、可升级、可监控、可恢复、可由真实操作员办赛的生产产品。Phase 7 分为：

- **7A W31–W41**：Packaging、Hardening、Scale、RC；
- **7B W42–W44**：Pilot、完整赛事演练、Production Ready。

---

## 2. Phase 7A 详细工作包

### P7.1 正式 Debian packages

- `natsume-server` 与 `natsume-client` 两个 Deb；
- real Rust binaries、Web assets、固定 Caddy；
- debconf/preseed Server endpoint；
- build-time site namespace/public trust roots；
- sysusers/tmpfiles/D-Bus/browser/Home/status assets；
- system-wide XDG Autostart entry与user-level shadow guard；明确不存在Session Agent systemd user unit；
- Agent Slint feature tree、ELF/DT_NEEDED、transitive package dependencies与forbidden runtime/executable scan；
- exactly expected units；
- no Identity Guard service；
- no systemd credentials；
- no runtime download；
- install/reinstall/upgrade/interrupted upgrade/remove/purge/reboot。

### P7.2 Release pipeline 与供应链

- locked builds；
- checksums/signatures；
- SBOM、license inventory、security scan；
- Caddy version/module/digest verification；
- signed offline APT repo；
- provenance/release manifest/changelog；
- artifact retention；
- rollback instructions。

### P7.3 Upgrade 与 data migration

- Server schema/vault AAD/key version；
- Client DB/vault migration；
- identity file/root key/endpoint/site namespace/public roots preservation；
- Device/Gateway cert records；
- pending Gateway certificate request/Command recovery；
- Caddy/systemd replacement；
- interrupted upgrade；
- no ongoing pre-v2.6 compatibility layer。

### P7.4 Security hardening

- threat model review；
- systemd sandboxing；
- file/dir/symlink/TOCTOU tests；
- secret canary scans；
- Enrollment request rate limit 与 authenticated Gateway CSR/issuer rate limit；
- certificate/revocation/profile tests；
- no 0-RTT；
- HTTP/CSV/proto/D-Bus fuzz；
- dependency/advisory review；
- optional host network policy。

### P7.5 Observability、Backup 与 Operations

- dashboards/alerts；
- audit export；
- Server DB/root-key separated backup/restore；
- PKI backup/restore；
- Client vault corruption/factory reset；
- pending/stuck Gateway certificate request visibility；
- certificate/readiness/secret/session/home dashboards；
- log retention/disk capacity。

### P7.6 Capacity 与 soak

- 2,000 sustained mTLS sessions；
- 200 active Commands；
- reconnect storm；
- bulk SYNC_STATE including Gateway issuance pacing；
- bulk human SYNC_SECRET；
- signer semaphore/rate limits；
- SQLite writer/WAL/SSE；
- Web 2,000 rows；
- long-duration soak；
- DOMjudge/Caddy data-plane load。

### P7.7 Runbooks 与培训

- fresh deployment/reset while preserving site identity/public roots；
- Server IP/port and control certificate；
- Device-only Enrollment/conflict/rekey；
- Gateway certificate request failure/reissue via SYNC_STATE；
- CSV correction/reimport；
- Device replacement no merge/split；
- password change + human secret sync；
- Caddy/LKG recovery；
- Session stale unlock；
- Home recovery；
- backup/restore/upgrade；
- incident/audit export。

---

## 3. Phase 7B Pilot 与演练

### P7.8 桌面支持矩阵与长期回归

- GNOME/GDM/Wayland为主支持组合；
- GNOME/GDM/X11在发行版提供时；
- Xfce或MATE/LightDM/X11为Display Manager独立性组合；
- 每个组合执行login/logout/relogin、Agent ready+hidden、lazy Binding window、Agent crash/lease timeout/session replacement、display disconnect、focus denied、notification absent、Browser launch、lock/unlock/terminate、Home reset；
- package升级后desktop entry、无user-unit约束、Slint feature和binary依赖闭包不漂移；
- 新desktop adapter必须通过ADR、fixture、VM/image和真实硬件矩阵。

### Pilot 1：10–30 Devices

- interactive/preseed install；
- mixed hardware Machine ID/Enrollment；
- 验证 Enrollment 后 Gateway credential absent；
- CSV import/reimport；
- binding；
- first SYNC_STATE triggers Gateway QUIC issuance；
- human SYNC_SECRET；
- password change；
- Server offline/reboot；
- desktop lock/Home；
- configured-disk copy；
- replacement workflow。

### Pilot 2：100–300 Devices

- concurrent Device-only Enrollment/approval；
- bulk SYNC_STATE with signer pacing；
- bulk human SYNC_SECRET；
- network partitions/reconnect；
- request result loss/idempotent recovery；
- operator shift handoff；
- alerts/runbooks；
- package update；
- audit/backup restore。

### Full dress rehearsal

```text
initialize fresh Server DB/vault/PKI
→ provision Server control leaf and Origin Intermediate
→ install/configure Clients
→ import full seat/account/password CSV
→ enroll/approve and issue Device Identity certificates only
→ establish mandatory-mTLS QUIC
→ bind Seats
→ explicit bulk SYNC_STATE
→ each Device obtains/reuses Gateway certificate over QUIC
→ verify visual BLOCKED local HTTPS
→ explicit bulk human SYNC_SECRET
→ readiness pass
→ contestants login/submit
→ desktop lock/unlock
→ password revision and controlled secret resync
→ failed Device replacement via unbind/revoke/delete/re-enroll/rebind
→ new Device SYNC_STATE obtains new Gateway certificate
→ Home Reset
→ Server short outage and Client reboot
→ configured-disk copy detection/reset
→ audit/export/backup
→ teardown/reset for next contest
```

演练禁止：手工改库、匿名 Gateway issuance、通过 Enrollment 返回 Gateway cert、明文密码、临时自签 Caddy cert、跳过 mTLS/PKI 校验。

---

## 4. 时间安排

### W31–W34

- package manifests/units/debconf；
- upgrade migrations；
- observability/backup；
- runbook drafts。

### W35–W38

- security hardening/fuzz/canary；
- 2,000/200 capacity；
- bulk Gateway issuance/signing pressure；
- soak。

### W39–W41

- package matrix；
- backup/restore drills；
- non-author runbook tests；
- RC defect close；
- G6 review。

### W42

- Pilot 1；
- operational fixes only，no architecture expansion。

### W43

- Pilot 2；
- release candidate refresh if required。

### W44

- Full dress rehearsal；
- operator sign-off；
- production freeze；
- G7 review。

---

## 5. 交付物

- signed Server/Client Deb；
- signed offline APT repo；
- SBOM/license/security/provenance reports；
- upgrade/rollback artifacts；
- dashboards/alerts；
- backup/restore evidence；
- performance/soak reports；
- complete runbooks/training material；
- Pilot reports；
- full rehearsal report；
- release notes/known limitations；
- G6/G7 evidence bundles。

---

## 6. 验证矩阵

| 场景 | 预期 |
|---|---|
| clean install/preseed | endpoint/site trust correct, no secret preseeded |
| upgrade with pending Command/Gateway request | deterministic resume/fail closed |
| package contents | no private key/password/runtime vault/cache |
| 2,000 fresh SYNC_STATE requiring certs | bounded signer/dispatcher load, no duplicate cert storm |
| result loss/reconnect storm | idempotent certificate result |
| Server DB restore without root key | fail closed |
| root key restore with DB | key-check and service recovery |
| Client copied to different hardware | reset before vault/Caddy |
| full rehearsal Enrollment | only Device cert returned |
| full rehearsal first config sync | Gateway cert over mTLS QUIC |
| operator password update | explicit human secret sync only |
| Session Agent GUI support matrix | GNOME Wayland、LightDM/X11、greeter拒绝、ready+hidden、lazy Slint window、user-level shadow、focus denied、crash/relogin |
| lock/unlock | Caddy unchanged |
| Server outage/reboot | steady Device recovers locally |

---

## 7. 风险与发布规则

| 风险 | 规则 |
|---|---|
| RC 后架构变更 | 重新进入相应 Phase/Gate，不在 Pilot 热修补绕过 |
| signer capacity不足 | 限流/分批/预热 SYNC_STATE；不放宽授权 |
| runbook 只能由作者执行 | G6 不通过，必须由非作者复现 |
| Critical/High security defect | release blocker |
| full rehearsal 需手工 DB edit/plaintext secret | G7 失败 |
| package runtime download | release blocker |
| Enrollment 返回 Gateway cert 回归 | schema/test/rehearsal release blocker |

---

## 8. G6 Gate 清单

- [ ] formal packages clean/upgrade/purge/reboot 通过；
- [ ] XDG Autostart direct Agent在全部支持桌面组合通过，package无Session Agent user unit；
- [ ] user-level shadow guard与Agent-missing Browser gate package/VM test通过；
- [ ] Agent Slint feature/依赖闭包无Qt/tray/interpreter/live-preview/MCP/testing及额外GUI runtime，且无外部GUI helper调用；
- [ ] SBOM/license/security/provenance/signing 完整；
- [ ] secret canary/package scans clean；
- [ ] backup/restore/key handling 演练；
- [ ] pending Gateway request upgrade/recovery 通过；
- [ ] 2,000/200/reconnect/bulk signer/soak 通过；
- [ ] runbooks 由非作者通过；
- [ ] 无 Critical/High unresolved；
- [ ] RC artifacts 从 tag 可重复产生；
- [ ] G6 decision 已签署。

## 9. G7 Gate 清单

- [ ] Pilot 1/2 通过；
- [ ] full rehearsal 无 architecture bypass/manual DB edit/plaintext secret；
- [ ] full rehearsal至少在GNOME Wayland与LightDM/X11验证Agent初始无窗口并完成Slint Binding Prompt；
- [ ] Enrollment only Device certificate 被现场验证；
- [ ] first Gateway certificate 由 SYNC_STATE over mTLS QUIC 取得；
- [ ] operator sign-off；
- [ ] all prior Gate evidence linked；
- [ ] release notes/known limitations/rollback/factory reset 完整；
- [ ] production tag/packages/repository frozen；
- [ ] post-event feedback process defined；
- [ ] G7 decision 已签署。
