# Phase 0 实验室与资产清单

> 状态：`DRAFT-STEP0`，目前无已验证硬件或目标环境证据  
> Phase 0 窗口：2026-07-23 至 2026-08-12  
> Owner 角色：以 `docs/supported-platform.md` 的角色字典为准  
> 规则：不得编造 IP、主机、硬件型号、序列号、fixture 或 PASS 结论。  
> 状态边界：字段级架构/环境冻结使用 `docs/supported-platform.md` 的 `ARCH-*` / `ENV-*` 词汇；资产行只使用本文件的可用性状态。

## 资产可用性状态定义

| 状态 | 含义 |
|---|---|
| `UNASSIGNED` | 仅建立槽位，尚无资产 |
| `UNFROZEN` | 已有候选资产，但规格或时间窗未确认 |
| `RESERVED` | Owner 和使用时间窗已确认 |
| `ONLINE` | 可运行探针，不代表探针通过 |
| `OFFLINE` | 当前不可用 |

## 1. 总览

| 类别 | 最小需求 | 当前状态 | Owner | 到位截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 物理 Client | 6 台、2 个 OEM/主板系列、SATA+NVMe | 0/6，`UNASSIGNED` | `ROLE_LAB_HARDWARE` | 2026-08-01 | Probe D、G0-011 |
| Server 目标 OS VM | 至少 1 台，systemd 可用 | `UNASSIGNED` | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe A/F、G0-003/012 |
| Client 目标 OS VM | 至少 1 台，可安装 Deb | `UNASSIGNED` | `ROLE_CLIENT_PLATFORM` | 2026-07-29 | Probe C/F、G0-012 |
| Desktop/Kiosk 环境 | 至少 2 套：GNOME/GDM/Wayland 与 LightDM 启动的目标 X11 desktop | `UNASSIGNED` | `ROLE_DESKTOP` | 2026-08-01 | Probe E/F、G0-010/012 |
| 实验室网络 | Server IP literal、TCP/UDP port、Client 地址段 | `UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | Probe A/B/F、G0-002/003/006 |
| Caddy/DOMjudge | 固定 Caddy 与 DOMjudge contract 环境 | `UNASSIGNED` | `ROLE_CADDY_SUPPLY` / `ROLE_DOMJUDGE` | 2026-08-01 | Probe C |

WSL、普通开发机或虚拟硬件序列号不得充当目标 OS 或六台物理 Machine ID 证据。

## 2. 物理工作站槽位

入库 fixture 只能保存匿名化证据、candidate UUID、quality 和 typed status；不得提交原始 serial、private key 或真实密码。

| ID | 资产标签 | OEM/主板系列 | 存储 | Owner | 可用窗口 | 状态 | Fixture 路径 | 阻塞 |
|---|---|---|---|---|---|---|---|---|
| `HW-01` | `TBD` | `TBD` | `TBD` | `ROLE_LAB_HARDWARE` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | 未产生 | Probe D/G0-011 |
| `HW-02` | `TBD` | `TBD` | `TBD` | `ROLE_LAB_HARDWARE` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | 未产生 | Probe D/G0-011 |
| `HW-03` | `TBD` | `TBD` | `TBD` | `ROLE_LAB_HARDWARE` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | 未产生 | Probe D/G0-011 |
| `HW-04` | `TBD` | `TBD` | `TBD` | `ROLE_LAB_HARDWARE` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | 未产生 | Probe D/G0-011 |
| `HW-05` | `TBD` | `TBD` | `TBD` | `ROLE_LAB_HARDWARE` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | 未产生 | Probe D/G0-011 |
| `HW-06` | `TBD` | `TBD` | `TBD` | `ROLE_LAB_HARDWARE` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | 未产生 | Probe D/G0-011 |

覆盖条件全部保持开放：

- [ ] 已验证物理工作站至少 6 台；
- [ ] 已覆盖至少 2 个 OEM 或主板系列；
- [ ] 已覆盖 SATA；
- [ ] 已覆盖 NVMe；
- [ ] 已预约至少一次 configured-disk copy 演练。

## 3. VM 与桌面环境

| ID | 角色 | OS/版本 | systemd/DM/DE | 规格 | Owner | 可用窗口 | 状态 | Probe |
|---|---|---|---|---|---|---|---|---|
| `ENV-SERVER-01` | Server package/IP-SAN | `ENV-UNFROZEN` | systemd `ENV-UNFROZEN` | `ENV-UNFROZEN` | `ROLE_SERVER_PLATFORM` | 2026-07-29 至 2026-08-12 | `UNASSIGNED` | A/F |
| `ENV-CLIENT-01` | Client package/Caddy | `ENV-UNFROZEN` | systemd `ENV-UNFROZEN` | `ENV-UNFROZEN` | `ROLE_CLIENT_PLATFORM` | 2026-07-29 至 2026-08-12 | `UNASSIGNED` | C/F |
| `ENV-CLIENT-02` | Upgrade/reinstall | `ENV-UNFROZEN` | systemd `ENV-UNFROZEN` | `ENV-UNFROZEN` | `ROLE_RELEASE` | 2026-08-05 至 2026-08-12 | `UNASSIGNED` | F |
| `ENV-DESKTOP-01` | Agent/lock/Home | `ENV-UNFROZEN` | GNOME + GDM + Wayland | `ENV-UNFROZEN` | `ROLE_DESKTOP` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | E/F |
| `ENV-DESKTOP-02` | Agent/lock/Home | `ENV-UNFROZEN` | LightDM + target X11 desktop | `ENV-UNFROZEN` | `ROLE_DESKTOP` | 2026-08-01 至 2026-08-08 | `UNASSIGNED` | E/F |

Desktop 环境关闭 G0-010 前必须产生：

- [ ] 两套桌面均由 `/etc/xdg/autostart/org.natsume.SessionAgent.desktop` 直接启动同一 binary；
- [ ] 初始状态无可见窗口，typed trigger 后懒创建 Slint 窗口并可再次隐藏；
- [ ] package/运行态均不存在 `natsume-session-agent.service` user unit；
- [ ] 中文/IME、HiDPI、focus denied 与依赖闭包证据；
- [ ] 一次真实 lock；
- [ ] 一次真实 unlock；
- [ ] Caddy Admin 调用次数为 0 的证据；
- [ ] Caddy config hash/epoch/status 未变化的证据。

## 4. 网络与安装输入

| 项目 | 当前值 | 状态 | Owner | 截止日期 | 阻塞 |
|---|---|---|---|---|---|
| 地址族 | IPv4 必选；IPv6 是否作为主支持待定 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | Probe A |
| Server IP literal | 未分配 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | Probe A/B/F、G0-002/003/006 |
| Server port | 候选 `8443`，未验证 | `ENV-PROPOSED` | `ROLE_SERVER_PLATFORM` | 2026-07-29 | Probe A |
| Client 地址段 | 未分配 | `ENV-UNFROZEN` | `ROLE_LAB_NETWORK` | 2026-07-29 | Probe B lab smoke |
| DOMjudge upstream | 未分配 | `ENV-UNFROZEN` | `ROLE_DOMJUDGE` | 2026-08-01 | Probe C |
| 出网策略 | package install/postinst 禁止下载；架构约束，不表示环境签收 | `ARCH-FROZEN` | `ROLE_RELEASE` | — | G0-013 |

IP-SAN 决策未被 ADR 替代前，禁止使用 TOFU 或 dangerous verifier。

## 5. PKI 和公开物料登记

| ID | 物料 | 允许登记内容 | Owner | 截止日期 | 状态 |
|---|---|---|---|---|---|
| `MAT-CONTROL-ROOT-PUB` | Control Trust Root 公钥证书 | 文件路径和 SHA-256 指纹 | `ROLE_PKI` | 2026-08-01 | `ENV-UNFROZEN` |
| `MAT-ORIGIN-ROOT-PUB` | Local Origin Root 公钥证书 | 文件路径和 SHA-256 指纹 | `ROLE_PKI` | 2026-08-01 | `ENV-UNFROZEN` |
| `MAT-SERVER-IP-SAN` | 测试 Server leaf | SAN、SPKI 指纹和有效期；不登记私钥 | `ROLE_PKI` | 2026-08-01 | `ENV-UNFROZEN` |
| `MAT-SITE-NAMESPACE` | `fleet_namespace_uuid` | 测试 namespace 值和来源 | `ROLE_PACKAGING` | 2026-08-01 | `ENV-UNFROZEN` |
| `MAT-PRESEED` | Client preseed fixture | 非秘密 IP/port | `ROLE_PACKAGING` | 2026-08-01 | `ENV-UNFROZEN` |

## 6. Probe 与环境映射

| Probe | 环境 | 预期报告路径 | 当前状态 | 阻塞输入 |
|---|---|---|---|---|
| A | `ENV-SERVER-01`、`ENV-CLIENT-01`、实验室网络 | `docs/probes/a-ip-san.md` | 未执行 | Server OS/IP、PKI |
| B | CI harness；可选实验室网络 smoke | `docs/probes/b-certificate-ladder.md` | 未执行 | PKI test material、契约实现 |
| C | `ENV-CLIENT-01`、Caddy、DOMjudge upstream | `docs/probes/c-caddy-domjudge.md` | 未执行 | Caddy/Browser/DOMjudge |
| D | `HW-01` 至 `HW-06` | `docs/probes/d-machine-identity.md` | 未执行 | 六台物理机 |
| E | `ENV-DESKTOP-01` | `docs/probes/e-session-home.md` | 未执行 | Desktop、filesystem |
| F | `ENV-SERVER-01`、`ENV-CLIENT-01/02` | `docs/probes/f-package-systemd.md` | 未执行 | 目标 OS、nFPM、Caddy |

报告路径是后续计划位置，不表示文件或证据已经存在。

## 7. 时间窗

| 周期 | 日期 | 目标 |
|---|---|---|
| W1 | 2026-07-23 至 2026-07-29 | 冻结 OS、IP、Caddy、Desktop、Browser、DOMjudge 候选；VM 到位 |
| W2 | 2026-07-30 至 2026-08-05 | Package/Caddy 预跑；物理采集开始；Probe B 主窗口 |
| W3 | 2026-08-06 至 2026-08-12 | 六台 fixture、Desktop/Home、package lifecycle、Gate evidence |

## 8. 缺口登记

| ID | 缺口 | Owner | 截止日期 | 影响 |
|---|---|---|---|---|
| `GAP-LAB-001` | 目标 Server/Client OS VM 未建立 | `ROLE_SERVER_PLATFORM` / `ROLE_CLIENT_PLATFORM` | 2026-07-29 | G0-002/003/012 |
| `GAP-LAB-002` | Server IP literal 未分配 | `ROLE_LAB_NETWORK` | 2026-07-29 | G0-002/003/006 |
| `GAP-LAB-003` | 物理机 0/6 | `ROLE_LAB_HARDWARE` | 2026-08-01 | G0-011 |
| `GAP-LAB-004` | Desktop/lock API 未冻结 | `ROLE_DESKTOP` | 2026-08-01 | G0-010 |
| `GAP-LAB-005` | Caddy/DOMjudge 环境未冻结 | `ROLE_CADDY_SUPPLY` / `ROLE_DOMJUDGE` | 2026-08-01 | Probe C |

逾期缺口必须在 G0 checklist 中保持 `BLOCKED-INPUT`；不得以模板或 mock 硬件证据关闭。
