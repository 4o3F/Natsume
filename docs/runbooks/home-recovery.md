# Home Recovery

> 适用：Home prepare/cleanup 中断、mount/copy 不确定、disk full、reboot 后残留  
> 关键不变量：`INV-INPUT-01`、`INV-PRIVILEGE-01`、`INV-SESSION-01`

无法证明 Home 安全时不得启动受管 session。

## 1. 前提

- 当前 Device、Seat、assignment revision、SessionEpoch、HomeEpoch 已记录；
- 部署选择的 backend 已确认：OverlayFS 或 staged-copy；
- 不允许运行时切换 backend；
- contest user、template version、固定路径 policy 已确认；
- 当前 session 已停止或不访问待恢复 Home；
- 有足够磁盘和只读 evidence 备份。

## 2. 立即行动

1. 阻止新 session start；
2. 停止当前 Home transition；
3. 记录 journal、mount table、process、disk usage、owner/mode、template hash；
4. 不要手工删除随机路径；
5. 确认 Helper 只使用 package-derived path/UID；
6. 保存错误码和 command/audit correlation。

## 3. 判断 HomeEpoch

- journal 指向当前 epoch，步骤明确：按 backend 恢复；
- journal 与 active mount/copy不一致：进入人工检查；
- 存在陈旧 epoch：不得用其 cleanup 操作当前 Home；
- epoch缺失且存在残留：视为无法证明安全；
- 多个 active epoch：停止并标记 conflict。

## 4. OverlayFS

检查：

- lower/template version；
- upper/work/mount path；
- mount namespace；
- mount owner/options；
- 活跃进程/open files；
- reboot后残留；
- filesystem/kernel支持；
- disk/inode。

恢复：

1. 停止 contest session/process；
2. 验证所有路径来自固定 policy；
3. 按 journal 判断完成 prepare、unmount/cleanup 或 rollback；
4. 不得在 mount状态未知时删除 upper/work；
5. 完成后验证无残留 mount；
6. 重新 prepare 新 HomeEpoch；
7. 验证 ownership/mode/template；
8. 再允许 session。

若目标环境证明 OverlayFS 不适用，停止部署并通过 ADR选择 staged-copy，不在运行时 fallback。

## 5. Staged-copy

检查：

- template source/hash；
- staging destination；
- copy progress/journal；
- final destination；
- owner/mode；
- disk/inode；
- 活跃进程；
- atomic rename边界。

恢复：

1. 停止 contest session/process；
2. 保留不确定 staging hash/evidence；
3. 若 journal证明尚未切换，删除受控 staging并重建；
4. 若切换结果不确定，比较 marker/hash/owner；
5. 不能证明完整则隔离 current Home，创建新的 clean staging；
6. 原子切换；
7. 验证 template和权限；
8. 清理隔离副本按 retention；
9. 再允许 session。

## 6. Disk full / corruption

- 释放空间不能删除 vault、identity、journal或当前 evidence；
- 优先清理批准的 cache/stale staging；
- 检查 filesystem error；
- 必要时从只读 backup恢复 template；
- 恢复后重跑完整 Home verify；
- 不能只因命令 exit 0 就开始 session。

## 7. 成功判定

- 只有一个 current HomeEpoch；
- journal和实际状态一致；
- 选定 backend未改变；
- template version/hash正确；
- owner/mode正确；
- 无陈旧 mount/staging；
- 新 session可安全启动；
- Observed更新；
- audit/evidence完整。

## 8. Evidence

```text
BACKEND=
DEVICE_PK=
SEAT=
ASSIGNMENT_REVISION=
OLD_HOME_EPOCH=
NEW_HOME_EPOCH=
TEMPLATE_VERSION=
JOURNAL_STATE=
MOUNTS_OR_STAGING=
DISK_STATE=
RECOVERY_ACTION=
VERIFY_RESULT=
SESSION_RELEASED=
OBSERVED_SEQUENCE=
OWNER=
REVIEWER=
```
