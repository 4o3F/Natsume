# Phase 3 状态

> 状态：`DRAFT-STEP0`
> 最后更新：2026-08-16
> G3：`OPEN`（实现推进中；证据随包登记）

Phase 3（Identity & Enrollment）启动分解。条目通过需可定位 evidence；partial pass 记为未通过。

## 工作包分解（启动定义，2026-08-16）

| WP | 内容 | 状态 |
|---|---|---|
| WP1 | machine-identity 整机组合配方冻结 + claim 层 2-of-3 + 词表统一（ADR-0032 修订） | `DONE`（`6b40ab8`；26 项决策表/golden 测试，缺失标记字节已钉死） |
| WP2 | Server enrollment 面：provisioning window open/close operator API（契约需新增冻结）、enrollment request 受理（同端口独立路由族）、`create_device` 同事务联合签发（Token + Gateway leaf）、`replace_device_credentials` approve-then-claim、`202` 幂等重投轮询、same-SPKI 自动批准 | `OPEN` |
| WP3 | Client：privileged raw collectors（DMI/disk 实读）、identity file 原子写、identity-first startup、凭据文件 | `OPEN` |
| WP4 | Client enrollment 流程接线 + 替换语义 | `OPEN` |

- WP2a（provisioning window operator API：open / close / read）已随本变更落地；WP2 总体状态保持 `OPEN`。

## WP2 启动待冻结面（设计项，非 owner 决策）

- provisioning window open/close 的 operator HTTP 面（启动时待冻结；现由上述 WP2a 按 §3.3 route 与 §3.6.4 audit registry 落地，保留本项作为启动记录）
- enrollment request 的 device 侧 wire（路径、`202` + 轮询语义、与 `approveEnrollment` 的资源关系）
- Gateway leaf 签发的服务端 CA 材料来源与存放（ADR-0033 权威；G0-IN-006：双根自签）

## 已登记待办

- G3 证据登记需含各包 head 的全绿 CI run（待 owner push）。
- fixture 决策表全路径证据依赖 G0-IN-005 硬件 fixture（BLOCKED-INPUT，工具已就绪）。
- WP2b 不写 `enrollment_requests.state = 'conflict'`：冻结文档只定义 different-SPKI live request 的稳定零写入拒绝，尚未定义该 persisted terminal state 的 writer。
- WP2b 的 credential replacement 旧连接 anomaly audit 等 WSS connection facts 落地后实现；本包禁止预建 WSS 或虚构 live-connection evidence。
