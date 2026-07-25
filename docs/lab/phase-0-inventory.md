# Phase 0 实验室与资产清单

> 状态：`DRAFT-STEP0`  
> 最后复核：2026-07-24  
> Phase 0 窗口：2026-07-23 至 2026-08-12  
> 当前没有已验证硬件或目标环境 evidence  
> 规则：不得编造 IP、主机、硬件型号、序列号、fixture 或 PASS

平台冻结状态见 [`../supported-platform.md`](../supported-platform.md)；Gate 机器状态见 [`../verification/registry.json`](../verification/registry.json)。

## 1. 资产状态

| 状态 | 含义 |
|---|---|
| `UNASSIGNED` | 只有槽位，无资产 |
| `UNFROZEN` | 有候选，但规格/时间窗未确认 |
| `RESERVED` | Owner 和可用窗口已确认 |
| `ONLINE` | 可以运行 Probe，不等于 Probe PASS |
| `OFFLINE` | 当前不可用 |
| `RETIRED` | 不再用于当前 Phase evidence |

## 2. 总览

| 类别 | 最小需求 | 当前状态 | Owner | 到位截止 | 阻塞 |
|---|---|---|---|---|---|
| 物理 Client | 6 台、2 个 OEM/主板系列、SATA+NVMe | `0/6 UNASSIGNED` | `ROLE_LAB_HARDWARE` | 2026-08-01 | Probe D / G0-011 |
| Server 目标 OS VM | 至少 1 台、systemd | `UNASSIGNED` | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe A/F |
| Client 目标 OS VM | 至少 1 台、可安装 Deb | `UNASSIGNED` | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe C/F |
| Desktop/Kiosk | GNOME/GDM/Wayland + LightDM/X11 | `UNASSIGNED` | `ROLE_DESKTOP` | 2026-08-01 | Probe E/F |
| 实验室网络 | Server IP、TCP/UDP port、Client 地址段 | `UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | Probe A/B/F |
| Caddy/DOMjudge | 固定 artifact 和 upstream contract | `UNASSIGNED` | `ROLE_CADDY_SUPPLY` / `ROLE_DOMJUDGE` | 2026-08-01 | Probe C |
| PKI test material | control/device/local origin test hierarchy | `UNASSIGNED` | `ROLE_PKI` | 2026-08-01 | Probe A/B/C |

WSL、普通开发机、虚拟硬件 serial 或 reference scaffold 不得充当目标环境 evidence。

## 3. 物理工作站槽位

Fixture 只保存匿名化候选、quality、typed result 和 derived ID；不得保存原始 serial、private key、真实 password 或完整 Machine Hardware ID。

| ID | 资产标签 | OEM/主板系列 | 存储 | Owner | 可用窗口 | 状态 | Fixture | 阻塞 |
|---|---|---|---|---|---|---|---|---|
| `HW-01` | 未分配 | 未分配 | 未分配 | `ROLE_LAB_HARDWARE` | 2026-08-01–08 | `UNASSIGNED` | 未产生 | Probe D |
| `HW-02` | 未分配 | 未分配 | 未分配 | `ROLE_LAB_HARDWARE` | 2026-08-01–08 | `UNASSIGNED` | 未产生 | Probe D |
| `HW-03` | 未分配 | 未分配 | 未分配 | `ROLE_LAB_HARDWARE` | 2026-08-01–08 | `UNASSIGNED` | 未产生 | Probe D |
| `HW-04` | 未分配 | 未分配 | 未分配 | `ROLE_LAB_HARDWARE` | 2026-08-01–08 | `UNASSIGNED` | 未产生 | Probe D |
| `HW-05` | 未分配 | 未分配 | 未分配 | `ROLE_LAB_HARDWARE` | 2026-08-01–08 | `UNASSIGNED` | 未产生 | Probe D |
| `HW-06` | 未分配 | 未分配 | 未分配 | `ROLE_LAB_HARDWARE` | 2026-08-01–08 | `UNASSIGNED` | 未产生 | Probe D |

覆盖条件：

- [ ] 至少 6 台物理工作站；
- [ ] 至少 2 个 OEM/主板系列；
- [ ] SATA；
- [ ] NVMe；
- [ ] placeholder/缺失/permission denied；
- [ ] duplicate/conflict；
- [ ] reboot/reinstall 稳定性；
- [ ] configured-disk copy 演练。

## 4. VM 和桌面

| ID | 角色 | OS/版本 | systemd/DM/DE | 规格 | Owner | 可用窗口 | 状态 | Probe |
|---|---|---|---|---|---|---|---|---|
| `ENV-SERVER-01` | Server package/IP-SAN | `ENV-UNFROZEN` | systemd unfrozen | unfrozen | `ROLE_SERVER_PLATFORM` | 2026-07-29–08-12 | `UNASSIGNED` | A/F |
| `ENV-CLIENT-01` | Client package/Caddy | `ENV-UNFROZEN` | systemd unfrozen | unfrozen | `ROLE_CLIENT_PLATFORM` | 2026-07-29–08-12 | `UNASSIGNED` | C/F |
| `ENV-CLIENT-02` | Upgrade/reinstall | `ENV-UNFROZEN` | systemd unfrozen | unfrozen | `ROLE_RELEASE` | 2026-08-05–12 | `UNASSIGNED` | F |
| `ENV-DESKTOP-01` | Agent/lock/Home | `ENV-UNFROZEN` | GNOME/GDM/Wayland | unfrozen | `ROLE_DESKTOP` | 2026-08-01–08 | `UNASSIGNED` | E/F |
| `ENV-DESKTOP-02` | Agent/lock/Home | `ENV-UNFROZEN` | LightDM/target X11 | unfrozen | `ROLE_DESKTOP` | 2026-08-01–08 | `UNASSIGNED` | E/F |

每套桌面必须产生：

- XDG direct launch；
- resident hidden + typed lazy UI；
- no user unit/descriptor；
- current logind session/singleton；
- 中文/IME、HiDPI、focus result；
- lock/unlock/terminate；
- Agent crash/display lost；
- Caddy call count/hash/generation/status 不变；
- selected Home backend；
- package runtime closure。

## 5. 网络和安装输入

| 项目 | 当前值 | 平台状态 | Owner | 截止 | 阻塞 |
|---|---|---|---|---|---|
| 地址族 | IPv4 必选；IPv6 主支持待定 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | A |
| Server IP literal | 未分配 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | A/B/F |
| Server port | 候选 `8443` | `ENV-PROPOSED` | `ROLE_SERVER_PLATFORM` | 2026-07-29 | A |
| Client 地址段 | 未分配 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | B |
| DOMjudge upstream | 未分配 | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` | 2026-08-01 | C |
| Caddy artifact | repo 候选 2.11.4 | `ENV-PROPOSED` | `ROLE_CADDY_SUPPLY` | 2026-07-29 | C/F |

## 6. PKI test material

| ID | 用途 | 状态 | 存储/Owner | Evidence |
|---|---|---|---|---|
| `PKI-TEST-CONTROL-ROOT` | Server control test root | `UNASSIGNED` | `ROLE_PKI` | 未产生 |
| `PKI-TEST-SERVER` | Server IP-SAN leaf | `UNASSIGNED` | `ROLE_PKI` | 未产生 |
| `PKI-TEST-DEVICE-ISSUER` | Device Identity issuer | `UNASSIGNED` | `ROLE_PKI` | 未产生 |
| `PKI-TEST-LOCAL-ORIGIN` | Gateway/local origin issuer | `UNASSIGNED` | `ROLE_PKI` | 未产生 |

测试 key 不得提交到仓库。只提交脚本、public certificate、fingerprint、serial、profile 和生成说明。

## 7. 预约记录模板

```text
ASSET_ID:
EXACT_MODEL_OR_IMAGE:
OWNER:
RESERVED_FROM:
RESERVED_TO:
NETWORK:
PURPOSE:
PROBES:
ACCESS_METHOD:
DATA_HANDLING:
KNOWN_LIMITATIONS:
```

槽位更新为 `RESERVED` 或 `ONLINE` 时必须补齐该信息。

## 8. Evidence 目录约定

仓库只保存可公开、脱敏、大小合理的 evidence。大型或敏感 artifact 使用受控外部存储，并在报告中保存 locator、hash 和访问策略。

建议：

```text
evidence/
  phase-0/
    probe-a/
    probe-b/
    probe-c/
    probe-d/
    probe-e/
    probe-f/
```

本目录不是要求在 `docs` 下提交真实 secret 或私有环境数据。
