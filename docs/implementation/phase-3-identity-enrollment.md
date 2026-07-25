# Phase 3 — Identity & Enrollment

> 计划：W13–W17  
> 入口：G2 PASS；目标 OS/硬件/PKI 输入已冻结  
> 退出：G3

## 1. 目标

让一台真实工作站能够以稳定 Machine Hardware ID 启动，安全打开 Client vault，通过 server-auth HTTPS 获得 Device Identity certificate，并建立可审计 Device lifecycle。

## 2. 工作包

### P3.1 Machine identity library

- candidate source types；
- normalization；
- placeholder/quality；
- conflict；
- deterministic UUIDv5；
- fleet namespace；
- anonymized fixture；
- pure tests。

### P3.2 Privileged collectors

- 最小 root capability；
- sysfs/SMBIOS/storage 等按证据准入；
- 原始 serial 不离开 helper边界；
- permission/missing/duplicate typed result；
- D-Bus policy；
- no network/secret。

### P3.3 Identity file and startup

固定 identity file：

```text
/var/lib/natsume/identity/machine-hardware-id
```

实现 [安全决策表](../security-recovery.md#6-身份启动决策)：

- first boot；
- match；
- unavailable；
- mismatch；
- vault decrypt failure；
- recovery required。

没有 Identity Guard service。

### P3.4 Client vault

- random root key；
- Machine ID-bound HKDF；
- versioned AEAD records；
- Device/Gateway/credential/LKG namespace；
- atomic write；
- wrong-key/tamper/crash；
- backup/recovery boundary；
- no automatic reset。

### P3.5 Endpoint and trust

- debconf/preseed IP literal + port；
- daemon validation；
- upgrade/reinstall preservation；
- explicit reconfigure；
- Server trust install；
- IP-SAN；
- no TOFU。

### P3.6 Device-only Enrollment

- Device Identity key/CSR；
- server-auth HTTPS；
- rate/size/version；
- Machine ID conflict；
- Device lifecycle；
- Device leaf/chain only；
- atomic local persistence；
- retry/idempotency；
- revoke/retire/delete/replacement。

### P3.7 PKI operations

- offline control root；
- Server control CSR/leaf；
- Device issuer/profile；
- local origin issuer boundary；
- certificate inventory；
- expiry/revocation；
- ceremonies/runbooks；
- test material separate from production。

## 3. 交付物

- Machine ID crate/collectors；
- 6+ physical fixtures；
- startup state machine；
- Client vault；
- endpoint config；
- Enrollment API/runtime；
- Device lifecycle UI/API；
- PKI provisioning and replacement runbooks；
- G3 evidence。

## 4. Definition of Done

- configured-disk copy fails closed；
- identity unavailable/mismatch does not open vault；
- decrypt failure does not create new Device；
- raw serial absent from repo/log/API；
- correct IP/CA works; wrong IP/CA/expiry fails；
- Enrollment has no Gateway material；
- Device key/leaf match validated；
- retries do not duplicate Device；
- replacement does not reuse key/vault/DevicePk；
- package upgrade preserves identity/endpoint；
- G3 decision signed。
