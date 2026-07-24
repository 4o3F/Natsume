# G0 Gate 检查清单

> 状态：`OPEN`，全部检查项未完成  
> Phase 0 窗口：2026-07-23 至 2026-08-12  
> 规则：缺少证据等同未通过；禁止“先通过后补证据”。

关联文档：

- `docs/requirements/phase-0.md`
- `docs/supported-platform.md`
- `docs/lab/phase-0-inventory.md`
- `docs/adr/0000-template.md`

## 30 秒结论

| 项目 | 当前状态 |
|---|---|
| G0 总体 | `OPEN`，通过计数 `0 / 15` |
| 输入门禁 | 多项 `BLOCKED-INPUT`；不计作 G0 PASS |
| 证书不变量 | 全部未勾选 |
| Top 阻塞 | 目标 OS VM、Server IP literal、Caddy pin、物理机 0/6、双桌面矩阵、XDG/Slint/lock API |

## 判定定义

| 判定 | 含义 |
|---|---|
| `OPEN` | 未执行或未提交完整证据 |
| `BLOCKED-INPUT` | 平台、实验室或供应链输入未冻结，不等于通过 |
| `PASS` | 已有可复现、可定位、经 reviewer 接受的证据 |
| `FAIL` | 证据显示不满足要求 |

G0-001 至 G0-015 均不可豁免，也不可标记 `N/A`。

## 1. Gate 检查项

| 完成 | ID | 检查项 | 主要 REQ | Probe | 当前判定 | 证据定位符 | Owner |
|---|---|---|---|---|---|---|---|
| [ ] | `G0-001` | clean checkout 真实 CI 全绿，无占位任务或缺工具跳过 | REQ-P0-001–003/010–013 | CI | `OPEN` | 未产生 | `ROLE_BUILD` |
| [ ] | `G0-002` | Client endpoint 交互/preseed 持久化并在升级中保留 | REQ-P0-041/042 | A/F | `OPEN` | 未产生 | `ROLE_PACKAGING` |
| [ ] | `G0-003` | Server IP-SAN 正反测试通过 | REQ-P0-050/051 | A | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-004` | 匿名 QUIC 在进入 Protobuf 前被拒绝，0-RTT 关闭 | REQ-P0-053 | B | `OPEN` | 未产生 | `ROLE_PROTOCOL` |
| [ ] | `G0-005` | Enrollment schema、DB、runtime 只包含 Device certificate material | REQ-P0-031/033/035/036/052 | B | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-006` | Device cert → mandatory-mTLS QUIC → mock `SYNC_STATE` Gateway CSR | REQ-P0-031、REQ-P0-032、REQ-P0-052–054 | B | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-007` | 无 active command、错误 generation/configuration 的 Gateway CSR 被拒绝 | REQ-P0-054/057 | B | `OPEN` | 未产生 | `ROLE_PROTOCOL` |
| [ ] | `G0-008` | 相同 request/SPKI 幂等，不同 SPKI conflict | REQ-P0-055 | B | `OPEN` | 未产生 | `ROLE_PROTOCOL` |
| [ ] | `G0-009` | CSR SAN 被忽略，certificate 使用 target SAN/hostname | REQ-P0-056 | B | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-010` | GNOME/GDM/Wayland 与 LightDM 启动的目标 X11 desktop 均通过 XDG Autostart 直接启动同一 Agent；初始 resident + hidden、typed trigger 懒弹窗、无 systemd user unit；desktop lock/unlock 不调用 Caddy Admin且不改变 config/hash/epoch/status；状态页无 `session_locked` | REQ-P0-034、REQ-P0-038、REQ-P0-039、REQ-P0-063、REQ-P0-066 | C/E/F | `OPEN` | 未产生 | `ROLE_DESKTOP` |
| [ ] | `G0-011` | 至少 6 台物理 Machine ID fixture，满足 OEM 和 SATA/NVMe 覆盖 | REQ-P0-061/062 | D | `OPEN` | 未产生；当前 0/6 | `ROLE_LAB_HARDWARE` |
| [ ] | `G0-012` | 空壳 Deb 在目标 OS 完成 install/reinstall/upgrade/remove/purge/reboot | REQ-P0-040/043/060/065 | C/F | `OPEN` | 未产生 | `ROLE_RELEASE` |
| [ ] | `G0-013` | 无 Identity Guard、systemd credentials、runtime download 或 secret leakage | REQ-P0-013/044–046 | F/CI | `OPEN` | 未产生 | `ROLE_SECURITY` |
| [ ] | `G0-014` | 所有高风险探针均有 ADR、owner、结论和残余限制 | REQ-P0-004/064/070/082 | A–F | `OPEN` | 未产生 | `ROLE_ARCHITECTURE` |
| [ ] | `G0-015` | Gate decision 已由架构、工程和 QA 签署 | REQ-P0-080/081 | — | `OPEN` | `docs/gates/g0-decision.md` 尚不存在 | `ROLE_GATE_G0` |

当前通过计数：`0 / 15`。

## 2. Step 0 输入门禁

`G0-IN-*` 由 Gate Chair 根据 `supported-platform` 与 `lab` 文档维护。`BLOCKED-INPUT` 只表示输入未签收，不关闭 G0-001 至 G0-015；Step 0 文档存在也不自动把输入门禁改为 `PASS`。

| 完成 | ID | 输入 | 截止日期 | 当前判定 | 阻塞 |
|---|---|---|---|---|---|
| [ ] | `G0-IN-001` | Server/Client 目标 OS、architecture、systemd 已冻结 | 2026-07-29 | `BLOCKED-INPUT` | G0-012 |
| [ ] | `G0-IN-002` | 实验室 Server IP literal 与 TCP/UDP port 已冻结 | 2026-07-29 | `BLOCKED-INPUT` | G0-002/003/006 |
| [ ] | `G0-IN-003` | Caddy version/modules/source/SHA-256 已冻结 | 2026-07-29 | `BLOCKED-INPUT` | G0-012/013 |
| [ ] | `G0-IN-004` | Browser、DOMjudge、GNOME/GDM/Wayland、LightDM+目标 X11 desktop、XDG Autostart、Slint runtime closure 与 lock API 已冻结 | 2026-08-01 | `BLOCKED-INPUT` | Probe C/E/F、G0-010 |
| [ ] | `G0-IN-005` | 六台物理硬件已到位并登记 | 2026-08-01 | `BLOCKED-INPUT` | G0-011 |
| [ ] | `G0-IN-006` | PKI test material 与 ownership 已登记 | 2026-08-01 | `BLOCKED-INPUT` | G0-003–009 |
| [ ] | `G0-IN-007` | 五份 Step 0 文档已创建；Requirements、platform、lab、ADR、checklist 的 ID 和术语仍需正式对齐签收 | Step 0 | `OPEN` | G0-014/015 |

## 3. 证书不变量复核

以下项目在 Step 0 全部保持未勾选，只有自动化和实验室证据完整后才能修改：

- [ ] Enrollment request、数据库 fixture 和响应无 Gateway CSR/SPKI/leaf/chain；
- [ ] Device Identity 与 Gateway certificate 就绪分别展示和判断；叙述别名 `READY-DEVICE-ID` / `READY-GATEWAY-CERT` 不得实现为新 wire/API/DB 字段；
- [ ] Gateway CSR 只存在于 mandatory-mTLS QUIC + active `SYNC_STATE`；
- [ ] 无通用 `CertificateIssueRequest` 或 `INSTALL_CERTIFICATE`；
- [ ] 无 TOFU、dangerous verifier、Identity Guard 或 systemd credentials；
- [ ] 匿名 QUIC 未进入 Protobuf parser。

任何一项证书不变量失败时，G0-004 至 G0-009 至少一项必须判定 `FAIL`，不得通过 waiver 关闭。

## 4. Probe Report 齐套性

| 完成 | Probe | 预期报告 | 当前状态 |
|---|---|---|---|
| [ ] | A | `docs/probes/a-ip-san.md` | 未创建 |
| [ ] | B | `docs/probes/b-certificate-ladder.md` | 未创建 |
| [ ] | C | `docs/probes/c-caddy-domjudge.md` | 未创建 |
| [ ] | D | `docs/probes/d-machine-identity.md` | 未创建 |
| [ ] | E | `docs/probes/e-session-home.md`（含 XDG direct launch、hidden/lazy UI、双桌面矩阵与 lock） | 未创建 |
| [ ] | F | `docs/probes/f-package-systemd.md` | 未创建 |

报告文件存在不等于 Probe 通过；必须同时提供结果、证据和 reviewer 结论。

## 5. 证据记录字段

未来 `g0-evidence.md` 的每条证据至少包含：

```text
G0_ID
REQ_IDS
JUDGEMENT=PASS|FAIL|BLOCKED-INPUT
CI_URL_OR_TEST
ARTIFACT_PATH
COMMIT_SHA
ENV_OR_HW_ID
OWNER
REVIEWER
DATE
LIMITATIONS
```

## 6. Gate 签署区

| 角色 | 姓名 | 日期 | 结论 |
|---|---|---|---|
| Architecture owner | | | |
| Engineering owner | | | |
| QA owner | | | |

总体结论：`OPEN`。

只有 15 项全部为 `PASS` 且独立 `g0-decision.md` 已签署时，才允许把总体结论改为 `PASS`。
