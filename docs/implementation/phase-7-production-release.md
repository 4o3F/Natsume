# Phase 7 — Production Release

> 计划：W38–W44  
> 入口：G6 PASS  
> 退出：G7 / production release decision

## 1. 目标

把已验证功能转化为可安装、可升级、可恢复、可观测并能在现场演练的发布。

## 2. 工作包

### P7.1 Production packaging

- final Server/Client Deb；
- versioning；
- signed checksums/artifact metadata；
- sysusers/tmpfiles/modes；
- systemd/D-Bus/XDG；
- Caddy；
- install/preseed/reconfigure；
- upgrade/reinstall/remove/purge；
- no runtime download；
- package content/runtime dependency scan。

### P7.2 Database and vault lifecycle

- backup；
- restore；
- schema upgrade；
- vault format/key rotation；
- rollback boundary；
- corrupted/partial backup；
- restore validation；
- retention and access control。

### P7.3 PKI operations

- production root/intermediate ceremony；
- Server certificate provisioning；
- Device/Gateway expiry/rotation/revocation；
- lost/compromised Device；
- audit inventory；
- offline key custody；
- emergency procedure。

### P7.4 Observability and support

- dashboards；
- stable error alerts；
- connection/command/observed freshness；
- Caddy/session/home；
- audit/outbox；
- disk/certificate expiry；
- log retention/redaction；
- support bundle without secrets。

### P7.5 Capacity and resilience

- expected fleet and concurrency；
- bulk sync；
- Server restart；
- network partition；
- Device reconnect storm；
- SQLite WAL/backup；
- disk pressure；
- package reboot；
- DOMjudge outage；
- offline steady state。

### P7.6 Runbooks and training

- all runbooks rehearsal；
- operator quick reference；
- admin recovery；
- destructive action approval；
- evidence capture；
- role assignment；
- incident communication；
- training completion。

### P7.7 Release rehearsal

至少一次 clean-site rehearsal：

1. prepare PKI/Server；
2. install packages；
3. import CSV；
4. enroll/replace Device；
5. bind；
6. sync state；
7. sync secret；
8. verify Caddy/DOMjudge；
9. start/lock/unlock/terminate session；
10. Home reset；
11. simulate Server/network/device faults；
12. backup/restore；
13. upgrade/rollback；
14. contest reset；
15. review audit and residual risk。

## 3. 交付物

- release artifacts/checksums；
- support matrix；
- backup/restore evidence；
- capacity report；
- security review；
- signed PKI records；
- rehearsed runbooks；
- training record；
- G7/release decision。

## 4. Definition of Done

- target environments `ENV-FROZEN`；
- clean install and upgrade/rollback pass；
- backup restore produces verified Server truth/vault/cert state；
- fleet load within accepted SLO；
- offline steady state and reconnect storm pass；
- no secret in support/log/export；
- all critical runbooks rehearsed by non-author；
- release artifact reproducibility and provenance recorded；
- residual risk accepted by named owner；
- G7 decision signed。

## 5. 非目标

- 通过 Phase 7 临时引入新架构范围；
- 无 evidence 的“现场应该可以”；
- 依赖单一开发者记忆；
- 用 database copy 代替完整 secret/PKI restore；
- 将 rollback 理解为自动 schema downgrade。
