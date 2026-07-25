# CSV Import

> 适用：首次初始化 Seat universe 或后续账号/密码更新  
> 关键不变量：`INV-SECRET-01`、`INV-STATE-01`、`INV-SECRET-02`

## 1. 前提

- 操作员拥有 CSV commit 权限；
- 输入来源和版本已确认；
- 文件为 UTF-8，可含 BOM；
- 列恰好为 `seat,account,password`；
- 不在聊天、工单或普通共享盘复制真实密码；
- Server vault、数据库备份和审计可用；
- 当前是否已冻结 Seat universe 已确认。

## 2. 上传和 Preview

1. 在 Preparation Center 创建 staging；
2. 上传单个 CSV；
3. 确认 Server 返回 staging ID/hash、行数和非秘密 validation summary；
4. 检查 invalid、duplicate、Seat set mismatch；
5. 检查 action 分类：unchanged/account/password/both；
6. 不截图或导出 password；
7. 核对输入文件 hash 和 preview revision；
8. 由第二位 reviewer 检查 Seat/account 变化。

Preview 不修改 Server truth，也不联系 Device。

## 3. Commit

1. 确认 staging 未过期且 hash 未变化；
2. 对首次 commit，确认 Seat universe 将永久冻结；
3. 对后续 commit，确认 Seat set 完全相同；
4. 明确提交；
5. 记录 operation/correlation；
6. 等待 Server domain transaction 完成；
7. 检查 AuditEvent、ChangeEvent 和 credential revision；
8. 检查没有自动创建 `SYNC_STATE`/`SYNC_SECRET` Command；
9. 检查 Target/Drift 只反映新 Server truth。

## 4. 失败处理

| 情况 | 行为 |
|---|---|
| parse/validation 失败 | 修正源文件，创建新 staging |
| Seat set mismatch | 停止；不得通过重命名/删除绕过 |
| concurrent commit conflict | 重新读取当前 truth，重新 preview |
| vault/database transaction 失败 | 确认旧 truth 完整，保存 correlation |
| UI 断线 | 按 staging ID 查询，不盲目重复 commit |
| secret leakage 怀疑 | 停止、隔离 artifact、按安全事件处理 |

不得直接编辑数据库、credential revision 或 staging ciphertext。

## 5. 成功判定

- committed import hash 可追踪；
- Seat universe 正确；
- changed/unchanged revision 正确；
- 密码未出现在 API response、browser storage、日志、审计或导出；
- 无自动远端副作用；
- operator 可以显式选择后续 state/secret sync。

## 6. Evidence

```text
IMPORT_ID=
STAGING_HASH=
ROW_COUNT=
FIRST_COMMIT=YES|NO
PREVIEW_SUMMARY=
COMMIT_CORRELATION=
AUDIT_EVENT=
TARGET_CHANGE=
AUTO_COMMAND_COUNT=0
OWNER=
REVIEWER=
```
