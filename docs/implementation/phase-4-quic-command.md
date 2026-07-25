# Phase 4 — QUIC & Command Runtime

> 计划：W18–W23  
> 入口：G3 PASS  
> 退出：G4

## 1. 目标

建立经过 Device certificate 认证的长期控制面，以及能够跨断线、重试和进程崩溃保持正确性的 durable Command runtime。

## 2. 工作包

### P4.1 Wire protocol

- exact wire version；
- ALPN；
- length/max frame；
- envelope/oneof；
- descriptor golden；
- unknown enum/version；
- protocol error；
- no general RPC。

### P4.2 mTLS QUIC

- Device Identity client auth；
- lifecycle/revocation validation；
- anonymous TLS rejection；
- 0-RTT off；
- one control stream；
- connection limits/timeouts；
- reconnect/backoff；
- no Enrollment fallback。

### P4.3 Connection registry

- one current connection per active Device policy；
- boot/connection identity；
- replacement race；
- heartbeat/freshness；
- shutdown/drain；
- stale connection rejection。

### P4.4 Server dispatcher

- Command durable before send；
- OperationTarget/Attempt；
- queue；
- retry/expiry/cancel；
- result reconciliation；
- multi-device fairness；
- restart recovery；
- redacted diagnostics。

### P4.5 Device journal

- receipt durable before ack；
- payload hash；
- idempotent duplicate；
- conflict；
- in-progress crash recovery；
- terminal replay；
- bounded retention/compaction；
- corruption fail-safe。

### P4.6 Observed

- typed complete snapshot；
- sequence/boot；
- freshness；
- ingest；
- no secrets；
- stable errors；
- reconnect resync。

### P4.7 Gateway request context skeleton

- active `SYNC_STATE` association；
- Device/command/generation/revision；
- request/SPKI idempotency；
- no real production certificate activation yet；
- integration test through mTLS。

## 3. 交付物

- protocol crate；
- Server/Device adapters；
- connection registry；
- dispatcher；
- journal；
- simulator；
- Observed；
- Gateway request test path；
- G4 evidence。

## 4. Definition of Done

- unauthenticated bytes never reach decoder；
- 0-RTT disabled；
- oversize/version/oneof failures deterministic；
- same ID/same hash idempotent；
- same ID/different hash conflict；
- receipt-before-crash recovery；
- reconnect returns prior terminal status；
- Server restart preserves queue；
- stale connection cannot report current Device；
- Observed no secrets and supports freshness；
- load/fault test meets Phase target；
- G4 decision signed。
