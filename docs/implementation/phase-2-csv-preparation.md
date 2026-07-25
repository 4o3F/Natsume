# Phase 2 — CSV Preparation

> 计划：W9–W12  
> 入口：G1 PASS  
> 退出：G2

## 1. 目标

实现一个严格、可预览、可回滚、秘密不外泄的 CSV 准备流程，把上传内容转换为 Server truth，而不产生任何自动远端副作用。

## 2. 工作包

### P2.1 Upload and staging

- 单文件；
- UTF-8/BOM；
- size/row/field limits；
- encrypted staging；
- owner/expiry/cleanup；
- 上传中断恢复；
- secret-safe errors。

### P2.2 Parser and normalization

精确列：

```text
seat,account,password
```

- header/extra/missing column；
- duplicate Seat；
- 空值/长度/字符规则；
- normalization；
- no XLSX/ODS/formula/mapping；
- table-driven/property tests。

### P2.3 Preview

Server 计算：

- unchanged；
- account changed；
- password changed；
- both changed；
- invalid；
- Seat set mismatch。

Preview 使用 staging revision/hash，不能在 commit 时悄悄重解析不同内容。

### P2.4 Commit

- first commit freezes Seat universe；
- later commit requires exact Seat set；
- atomic domain + credential metadata + vault + audit + outbox；
- credential revision only on actual password change；
- optimistic concurrency；
- retry idempotency；
- failure leaves prior truth intact；
- no Command creation。

### P2.5 Preparation Center

- upload；
- validation summary；
- row-level non-secret status；
- confirmation；
- commit progress；
- discard；
- no password echo；
- no browser persistence；
- accessible large table；
- recovery from session/network interruption。

### P2.6 Export

只允许非秘密导出，例如 Seat/account/current binding/metadata。任何密码、ciphertext、private key 或 recovery material 禁止导出。

## 3. 交付物

- parser/preview/commit；
- encrypted staging；
- Preparation Center；
- secret-safe API/OpenAPI；
- property/fuzz fixtures；
- audit/export；
- G2 evidence。

## 4. Definition of Done

- malformed、duplicate、extra columns、BOM、size limits 全覆盖；
- first commit 与 later exact set；
- concurrent preview/commit conflict；
- crash/rollback；
- secret scan；
- browser storage inspection；
- unchanged password 不增加 revision；
- commit 不创建 Operation/Command；
- Target/Drift 只在领域提交后变化；
- G2 decision 签署。

## 5. 非目标

- 自动 sync；
- Device 在线要求；
- XLSX/ODS；
- password export；
- 可配置列映射；
- 多 Event import。
