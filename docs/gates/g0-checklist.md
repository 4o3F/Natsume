<!-- GENERATED FILE: DO NOT EDIT DIRECTLY -->
<!-- Source: docs/verification/registry.json; regenerate with node docs/verification/render.mjs --write -->

# G0 Gate 检查清单

> 总体：`OPEN`，通过计数 `0 / 15`  
> Phase 0 窗口：2026-07-23 至 2026-08-12  
> 缺少证据等同未通过；禁止先通过后补证据。

关联：

- [Phase 0 requirements](../requirements/phase-0.md)
- [支持平台](../supported-platform.md)
- [实验室清单](../lab/phase-0-inventory.md)
- [Probe reports](../probes/README.md)
- [Verification Registry](../verification/README.md)

## Gate 条目

| 完成 | ID | 检查项 | Requirements | Probe | 判定 | Evidence | Owner |
|---|---|---|---|---|---|---|---|
| [ ] | `G0-001` | clean checkout 真实 CI 全绿，无占位任务或缺工具跳过 | `REQ-P0-001`、`REQ-P0-002`、`REQ-P0-003`、`REQ-P0-010`、`REQ-P0-011`、`REQ-P0-012`、`REQ-P0-013`、`REQ-P0-020`、`REQ-P0-021`、`REQ-P0-023`、`REQ-P0-030`、`REQ-P0-033`、`REQ-P0-080` | `CI` | `OPEN` | 未产生 | `ROLE_BUILD` |
| [ ] | `G0-002` | Client endpoint 交互/preseed 持久化并在升级中保留 | `REQ-P0-041`、`REQ-P0-042`、`REQ-P0-080` | `A`、`F` | `OPEN` | 未产生 | `ROLE_PACKAGING` |
| [ ] | `G0-003` | Server IP-SAN 正反测试和 TCP/UDP 同端口验证通过 | `REQ-P0-050`、`REQ-P0-051`、`REQ-P0-080` | `A` | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-004` | 匿名 QUIC 在进入 Protobuf 前被拒绝，0-RTT 关闭 | `REQ-P0-022`、`REQ-P0-030`、`REQ-P0-053`、`REQ-P0-080` | `B` | `OPEN` | 未产生 | `ROLE_PROTOCOL` |
| [ ] | `G0-005` | Enrollment schema、DB、runtime 只包含 Device certificate material | `REQ-P0-012`、`REQ-P0-013`、`REQ-P0-031`、`REQ-P0-033`、`REQ-P0-035`、`REQ-P0-036`、`REQ-P0-037`、`REQ-P0-052`、`REQ-P0-080` | `B` | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-006` | Device cert → mandatory-mTLS QUIC → mock SYNC_STATE Gateway CSR | `REQ-P0-031`、`REQ-P0-032`、`REQ-P0-037`、`REQ-P0-052`、`REQ-P0-053`、`REQ-P0-054`、`REQ-P0-080` | `B` | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-007` | 无 active command、错误 generation/configuration 的 Gateway CSR 被拒绝 | `REQ-P0-020`、`REQ-P0-022`、`REQ-P0-032`、`REQ-P0-054`、`REQ-P0-057`、`REQ-P0-080` | `B` | `OPEN` | 未产生 | `ROLE_PROTOCOL` |
| [ ] | `G0-008` | 相同 request/SPKI 幂等，不同 SPKI conflict | `REQ-P0-020`、`REQ-P0-032`、`REQ-P0-055`、`REQ-P0-080` | `B` | `OPEN` | 未产生 | `ROLE_PROTOCOL` |
| [ ] | `G0-009` | CSR SAN 被忽略，certificate 使用 Target SAN/hostname | `REQ-P0-032`、`REQ-P0-056`、`REQ-P0-080` | `B` | `OPEN` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-010` | 双桌面 XDG direct Agent、hidden/lazy UI、无 user unit；lock/unlock 不改变 Caddy | `REQ-P0-022`、`REQ-P0-034`、`REQ-P0-038`、`REQ-P0-039`、`REQ-P0-043`、`REQ-P0-063`、`REQ-P0-066`、`REQ-P0-080` | `C`、`E`、`F` | `OPEN` | 未产生 | `ROLE_DESKTOP` |
| [ ] | `G0-011` | 至少 6 台物理 Machine ID fixture，满足 OEM 和 SATA/NVMe 覆盖 | `REQ-P0-061`、`REQ-P0-062`、`REQ-P0-080` | `D` | `OPEN` | 未产生 | `ROLE_LAB_HARDWARE` |
| [ ] | `G0-012` | 空壳 Deb 在目标 OS 完成 install/reinstall/upgrade/remove/purge/reboot | `REQ-P0-003`、`REQ-P0-014`、`REQ-P0-038`、`REQ-P0-039`、`REQ-P0-040`、`REQ-P0-043`、`REQ-P0-060`、`REQ-P0-065`、`REQ-P0-080` | `C`、`F` | `OPEN` | 未产生 | `ROLE_RELEASE` |
| [ ] | `G0-013` | 无 Identity Guard、systemd credentials、runtime download 或 secret leakage | `REQ-P0-003`、`REQ-P0-013`、`REQ-P0-023`、`REQ-P0-038`、`REQ-P0-043`、`REQ-P0-044`、`REQ-P0-045`、`REQ-P0-046`、`REQ-P0-060`、`REQ-P0-066`、`REQ-P0-080` | `F`、`CI` | `OPEN` | 未产生 | `ROLE_SECURITY` |
| [ ] | `G0-014` | 所有高风险探针均有 ADR、owner、结论和残余限制 | `REQ-P0-004`、`REQ-P0-039`、`REQ-P0-064`、`REQ-P0-070`、`REQ-P0-080`、`REQ-P0-082` | `A`、`B`、`C`、`D`、`E`、`F` | `OPEN` | 未产生 | `ROLE_ARCHITECTURE` |
| [ ] | `G0-015` | Gate decision 已由架构、工程和 QA 签署 | `REQ-P0-004`、`REQ-P0-080`、`REQ-P0-081`、`REQ-P0-082` | — | `OPEN` | 未产生 | `ROLE_GATE_G0` |

## 输入门禁

`BLOCKED-INPUT` 不计作 Gate PASS。

| 完成 | ID | 输入 | 截止 | 阻塞 | 判定 | Evidence | Owner |
|---|---|---|---|---|---|---|---|
| [ ] | `G0-IN-001` | Server/Client 目标 OS、architecture、systemd 已冻结 | 2026-07-29 | `G0-012` | `BLOCKED-INPUT` | 未产生 | `ROLE_SERVER_PLATFORM` |
| [ ] | `G0-IN-002` | 实验室 Server IP literal 与 TCP/UDP port 已冻结 | 2026-07-29 | `G0-002`、`G0-003`、`G0-006` | `BLOCKED-INPUT` | 未产生 | `ROLE_LAB_NETWORK` |
| [ ] | `G0-IN-003` | Caddy version/modules/source/SHA-256 已冻结 | 2026-07-29 | `G0-012`、`G0-013` | `BLOCKED-INPUT` | 未产生 | `ROLE_CADDY_SUPPLY` |
| [ ] | `G0-IN-004` | Browser、DOMjudge、双桌面、XDG、Slint closure 与 lock API 已冻结 | 2026-08-01 | `G0-010` | `BLOCKED-INPUT` | 未产生 | `ROLE_DESKTOP` |
| [ ] | `G0-IN-005` | 六台物理硬件已到位并登记 | 2026-08-01 | `G0-011` | `BLOCKED-INPUT` | 未产生 | `ROLE_LAB_HARDWARE` |
| [ ] | `G0-IN-006` | PKI test material 与 ownership 已登记 | 2026-08-01 | `G0-003`、`G0-004`、`G0-005`、`G0-006`、`G0-007`、`G0-008`、`G0-009` | `BLOCKED-INPUT` | 未产生 | `ROLE_PKI` |
| [ ] | `G0-IN-007` | Step 0 文档、ID 和术语已正式对齐签收 | Step 0 | `G0-014`、`G0-015` | `OPEN` | 未产生 | `ROLE_ARCHITECTURE` |

## Probe 齐套性

| Probe | 主题 | 报告 | 状态 | Evidence |
|---|---|---|---|---|
| `PROBE-A` | IP-SAN 与 endpoint | [a-ip-san.md](../probes/a-ip-san.md) | `NOT-RUN` | 未产生 |
| `PROBE-B` | Enrollment → mTLS → Gateway CSR | [b-certificate-ladder.md](../probes/b-certificate-ladder.md) | `NOT-RUN` | 未产生 |
| `PROBE-C` | Caddy 与 DOMjudge | [c-caddy-domjudge.md](../probes/c-caddy-domjudge.md) | `NOT-RUN` | 未产生 |
| `PROBE-D` | Machine identity | [d-machine-identity.md](../probes/d-machine-identity.md) | `NOT-RUN` | 未产生 |
| `PROBE-E` | Session Agent、Desktop 与 Home | [e-session-home.md](../probes/e-session-home.md) | `NOT-RUN` | 未产生 |
| `PROBE-F` | Package 与 systemd | [f-package-systemd.md](../probes/f-package-systemd.md) | `NOT-RUN` | 未产生 |

## 不可豁免复核

- Enrollment request、DB 和 response 无 Gateway material；
- Gateway CSR 只在 mandatory-mTLS QUIC 的 active `SYNC_STATE`；
- anonymous QUIC 未进入 Protobuf decoder，0-RTT 关闭；
- CSR SAN 不授予权限；
- 无 TOFU、Identity Guard、systemd credentials 或 runtime download；
- Session lock/unlock 不调用或改变 Caddy；
- secret 不进入 API、日志、Observed、UI storage 或状态页。

失败时必须把关联 Gate 标记为 `FAIL`，不能用 waiver 关闭。

## Gate decision

总体规则：15 项全部 PASS 且独立 decision 已签署后才可关闭。

预期 decision 路径：`docs/gates/g0-decision.md`。该文件在正式签署前不应创建为 PASS decision；可使用 [decision template](g0-decision-template.md)。
