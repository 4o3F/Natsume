# Phase 5 — State, Gateway & Data Plane

> 计划：W24–W30  
> 入口：G4 PASS；Caddy/DOMjudge/PKI target environment frozen  
> 退出：G5

## 1. 目标

交付从非秘密 Target 到本地 Caddy READY 的完整显式流程，以及完全独立、人工触发的 secret sync。

## 2. 工作包

### P5.1 Target and Drift

- deterministic Target；
- configuration generation/hash；
- assignment/credential metadata；
- latest Observed；
- dimensioned Drift；
- operator preview；
- no automatic Command。

### P5.2 `SYNC_STATE`

- operator authorization；
- target snapshot freeze；
- durable Command；
- Device validation/staging；
- revision/generation stale rejection；
- atomic activation；
- Observed/Drift reconciliation。

### P5.3 Gateway key/certificate

- local key generation；
- CSR/SPKI；
- active Command binding；
- Server-derived SAN/profile/validity；
- idempotency/conflict；
- certificate validation；
- atomic vault persistence；
- expiry/renewal/revocation policy。

### P5.4 Caddy

- fixed binary/module/checksum；
- loopback HTTPS；
- BLOCKED 503；
- fixed upstream；
- typed activation plan；
- config validation；
- Unix socket access；
- atomic load；
- LKG；
- rollback/fault recovery；
- status page security。

### P5.5 `SYNC_SECRET`

- explicit human action；
- Server vault read；
- assignment/credential revision binding；
- encrypted Command；
- Client vault atomic update；
- redacted status/audit；
- stale/retry/crash；
- no automatic sync。

### P5.6 Operator views

- Target/Observed/Drift；
- Device identity vs Gateway readiness；
- Operation/Command；
- Caddy BLOCKED/READY；
- secret revision without value；
- recovery actions；
- SSE freshness。

## 3. 交付物

- production Target/Drift；
- `SYNC_STATE`；
- Gateway issuer path；
- Caddy generator/adapter/status page；
- LKG；
- `SYNC_SECRET`；
- operator workflows；
- DOMjudge lab evidence；
- G5 decision。

## 4. Definition of Done

- Enrollment never signs Gateway；
- wrong/absent active Command rejected；
- CSR SAN ignored；
- same request/SPKI idempotent, different SPKI conflict；
- bad key/cert/SAN/expiry cannot enter READY；
- Caddy load failure preserves LKG or BLOCKED；
- status page returns 503 and no secret/free-form error；
- secret sync is human-only and stale-safe；
- Device offline preserves verified steady state；
- Observed/Drift converge after reconnect；
- G5 decision signed。
