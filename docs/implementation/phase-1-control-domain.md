# Phase 1 — Control Domain

> 计划：W4–W8  
> 入口：G0 PASS  
> 退出：G1

## 1. 目标

建立 Server 端可持续演进的领域、持久化、授权、审计和 Web shell，使后续 CSV、Device control 和 Operation 都依赖清晰模块而不是一个巨型 Server handler。

## 2. 工作包

### P1.1 Persistence baseline

- SQLite WAL；
- migration runner；
- 空库和升级测试；
- table ownership；
- transaction helper；
- backup-compatible settings；
- no cross-module arbitrary writes。

概念实体见 [领域模型](../domain-model.md)。实际列和 index 由 migration 拥有。

### P1.2 Contest domain

- Seat/account/credential metadata；
- Device；
- Binding；
- revision/generation value objects；
- lifecycle rules；
- repository ports；
- domain tests。

不实现生产 CSV parser；使用 fixture/command 建立最小事实。

### P1.3 Server vault

- AEAD format；
- key loading；
- secret handle/use case；
- atomic write/rotation skeleton；
- redaction；
- wrong-key/tamper tests；
- backup/restore test fixture。

### P1.4 Auth/RBAC

最小角色：

- platform/admin；
- contest operator；
- read-only/auditor；
- secret-sync authorization。

实现：

- session lifecycle；
- CSRF/session protection；
- route/use-case authorization；
- audit actor identity；
- Web auth shell。

### P1.5 Audit and outbox

- AuditEvent schema；
- ChangeEvent/outbox；
- domain transaction atomicity；
- dispatcher/SSE consumer skeleton；
- redacted diff；
- retry/idempotency；
- retention/export policy skeleton。

### P1.6 Operation/Command persistence

- Operation、OperationTarget、Command、Attempt；
- 状态和值对象；
- repository；
- 归约；
- 不连接真实 Device；
- mock dispatcher；
- 普通 CRUD 不强制创建 Operation。

### P1.7 Operator API and Web shell

- generated OpenAPI；
- problem details/ErrorCode；
- navigation；
- auth；
- Device/Seat/Binding read model；
- Operation/Audit placeholders backed by real API；
- loading/empty/error/accessibility；
- SSE reconnect skeleton。

## 3. 交付物

- production migration set；
- module dependency tests；
- domain/application tests；
- Server vault format；
- RBAC matrix；
- Audit/outbox integration；
- OpenAPI + TS generated code；
- Web shell e2e；
- G1 evidence。

## 4. Definition of Done

- 领域 module 不依赖 Axum/SQL row/Quinn；
- transaction + audit + outbox 原子；
- secret 不进入 read models/logs；
- RBAC 在 use-case boundary 强制；
- stable ErrorCode 穷举；
- migration 空库/升级通过；
- Operation 归约可重算；
- Web 不复制 domain enum；
- restart 后 Server state 可恢复；
- G1 decision 签署。

## 5. 非目标

- 生产 CSV；
- Enrollment；
- QUIC；
- Device journal；
- Gateway/Caddy；
- Session/Home。
