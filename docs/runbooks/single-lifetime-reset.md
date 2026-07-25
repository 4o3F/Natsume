# Single-Lifetime Contest Reset

> 适用：当前一场竞赛结束后，将同一 Server 部署重置为下一次独立生命周期  
> 这是破坏性操作，需要双人批准  
> 关键决策：ADR-0009

Natsume 不在业务数据库中保存多个 Event。需要保留的审计/业务记录必须在 reset 前按批准策略导出和归档。

## 1. 停止条件

出现以下任一情况，不执行 reset：

- 当前竞赛仍可能恢复；
- 未完成最终审计/导出；
- 备份未验证；
- pending Command状态未知；
- Device/Session/Home仍在运行；
- certificate/revocation计划未确认；
- 归档位置和访问控制未批准；
- 没有回滚/恢复决策；
- 操作员无法确认将删除的范围。

## 2. 准备

1. 宣布 maintenance；
2. 禁止新 mutation/Command；
3. 等待或终止所有 Operation/Command到可解释状态；
4. 收集最终 Observed/Drift；
5. 终止受管 session；
6. 清理 Home；
7. 将 Client数据面置于批准的 BLOCKED/maintenance；
8. 导出非秘密运营数据和审计；
9. 完成 Server一致备份和 restore test；
10. 记录 certificate、Device、Seat、credential revision inventory；
11. 双人签署 reset scope。

## 3. Client 处理

根据下一场是否复用同一硬件和信任决定：

- Device identity 是否保留必须由部署策略明确；
- 旧 binding/credential/Gateway state必须清理或失效；
- Client vault中竞赛credential必须按批准方式销毁；
- 旧 Gateway certificate不得授权下一场 Target；
- Home必须clean；
- 不能通过删除 Machine identity伪装新设备；
- 若硬件更换，走 Device replacement。

推荐使用正式 Client reset command/use case；不要手工删除随机文件。

## 4. Server reset

正式 reset use case 应原子或可恢复地：

- 冻结当前 mutation；
- 生成 reset ID；
- 记录预重置 audit；
- 清理 Seat/account/credential、binding、Target、Observed业务视图、Operation/Command活动状态；
- 按 policy保留或归档 Device identity/certificate metadata；
- 销毁当前竞赛 Server vault secrets；
- 重置 Seat universe frozen marker；
- 清理 staging/outbox；
- 保留必要安全审计和 reset lineage；
- 生成空实例状态。

具体保留范围必须在实现和发布策略中固定，不能现场选择 nullable 特例。

## 5. 验证空状态

- 无 Seat/account/password/binding；
- 无 active Target/Drift/Operation/Command；
- 无可用旧 credential；
- Client不使用旧 Gateway/credential进入新 READY；
- operator/RBAC按 policy保留或重新初始化；
- audit可以定位 reset；
- backup仍可在隔离环境恢复旧生命周期；
- 新 CSV首次 commit重新冻结 Seat universe。

## 6. 回滚

Reset 完成后通常不做原地“撤销”。需要恢复旧竞赛时：

1. 隔离当前空/新实例；
2. 从已验证 backup恢复到独立环境；
3. 使用原相容 package/key custody；
4. 不与新生命周期 Device同时控制；
5. 记录恢复用途；
6. 完成后安全销毁或归档。

## 7. 成功判定

- 旧竞赛秘密不可用于新生命周期；
- 空实例符合初始化状态；
- Client/Home/Caddy无旧授权；
- 旧 backup可验证恢复；
- reset audit和双人签署完整；
- 没有引入 Event/phase兼容字段；
- 下一次 CSV按首次 commit语义运行。

## 8. Evidence

```text
RESET_ID=
OLD_LIFETIME_BACKUP=
BACKUP_RESTORE_TEST=
FINAL_AUDIT_EXPORT=
CLIENT_COUNT=
SESSION_HOME_CLEAN=
CERTIFICATE_ACTIONS=
SECRET_DESTRUCTION=
EMPTY_STATE_CHECK=
APPROVER_1=
APPROVER_2=
DATE=
LIMITATIONS=
```
