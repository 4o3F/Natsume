# Phase 0 Probe Reports

> 当前状态：全部 `NOT-RUN`  
> Probe 状态见 [phase-0-status](../gates/phase-0-status.md)

Probe 用于验证目标环境中的高风险假设。报告文件存在不等于通过；只有填写真实环境、步骤、结果、artifact、owner 和 reviewer 后，才能更新 registry。

| Probe | 主题 | 报告 |
|---|---|---|
| A | IP-SAN 与 endpoint | [`a-ip-san.md`](a-ip-san.md) |
| B | Enrollment → mTLS → Gateway CSR | [`b-certificate-ladder.md`](b-certificate-ladder.md) |
| C | Caddy 与 DOMjudge | [`c-caddy-domjudge.md`](c-caddy-domjudge.md) |
| D | Machine identity | [`d-machine-identity.md`](d-machine-identity.md) |
| E | Session Agent、Desktop 与 Home | [`e-session-home.md`](e-session-home.md) |
| F | Package 与 systemd | [`f-package-systemd.md`](f-package-systemd.md) |

## 报告最低字段

```text
PROBE_ID
STATUS=NOT-RUN|RUNNING|PASS|FAIL|BLOCKED-INPUT
COMMIT_SHA
ENVIRONMENT_OR_HW_IDS
EXACT_VERSIONS
DATE
OWNER
REVIEWER
RELATED_REQUIREMENTS
RELATED_GATES
PRECONDITIONS
STEPS
POSITIVE_RESULTS
NEGATIVE_RESULTS
ARTIFACTS
LIMITATIONS
FOLLOW_UP
```

## 证据规则

- 使用可重现命令、日志、测试输出、packet capture、certificate inspection 或 package manifest；
- secret、private key、原始硬件 serial、真实密码必须脱敏；
- 失败证据不得删除；
- 屏幕截图只作辅助，不能替代 machine-readable artifact；
- 报告中的 `PASS` 与 registry 状态必须同步；
- 环境变化后重新运行受影响 Probe；
- 一个 Probe 部分通过时整体保持 `RUNNING` 或 `FAIL`，不能只记录成功用例。
