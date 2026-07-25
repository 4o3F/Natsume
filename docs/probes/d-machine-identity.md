# Probe D — Machine Identity

> Status: `NOT-RUN`  
> Requirements: `REQ-P0-061`、`REQ-P0-062`  
> Gate: `G0-011`

## 目标

在至少 6 台物理工作站上证明 Machine ID source、质量、稳定性、冲突和 configured-disk-copy fail-closed 行为。

## 覆盖

- 至少 2 个 OEM 或主板系列；
- SATA；
- NVMe；
- placeholder；
- source 缺失；
- permission denied；
- 重复/conflict；
- reboot/reinstall；
- configured-disk copy。

## 数据保护

报告和 fixture 只保存：

- 匿名 hardware ID；
- source kind；
- normalized/hashed candidate；
- quality/status；
- derived UUID；
- environment metadata。

不得提交原始 serial、asset private data、Device key 或 vault。

## 步骤

1. 登记 `HW-01`–`HW-06`；
2. 在每台机器收集重复运行/reboot 结果；
3. 对可控 source 制造缺失、placeholder、permission denied；
4. 验证 conflict 不自动选择；
5. 将已配置系统盘复制到另一物理机；
6. 验证 identity mismatch/unavailable 在 vault 前 fail closed；
7. 验证删除 identity/vault 不是自动恢复路径；
8. 提交匿名 fixture 和 reviewer 结论。

## 结果

```text
STATUS=NOT-RUN
PHYSICAL_COUNT=0
OEM_FAMILIES=0
SATA=NO
NVME=NO
CONFIGURED_DISK_COPY=NOT-RUN
ARTIFACTS=
LIMITATIONS=
```
