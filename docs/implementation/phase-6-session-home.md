# Natsume V2 Phase 6 详细实施计划：D-Bus、Session 与 Home

> 架构基线：`Natsume_V2_Design_v2.5.md`  
> Roadmap 基线：`Natsume_V2_Implementation_Roadmap_v1.2.md`  
> 计划版本：Phase Plan v1.0  
> 基准窗口：W25–W34  
> Gate：G5  
> 前置依赖：Phase 5 的 Daemon、Command lanes、Gateway readiness

---

## 1. 阶段使命与边界

完成现场 OS 与桌面控制能力，同时保持 root、network、secret、desktop 和 Caddy 边界。Session lock/unlock 只控制桌面与 Agent gate，绝不修改 Caddy configuration、certificate、activation journal 或 runtime state。

---

## 2. 详细工作包

### P6.1 Local Control API

- `org.natsume.Privileged1`；
- `org.natsume.Device1`；
- typed values/errors/properties/signals；
- zbus codegen/introspection snapshot；
- no secret types/free paths/shell strings/arbitrary UID/unit；
- compatibility/golden tests。

### P6.2 Privileged Helper hardening

- root + `PrivateNetwork=yes`；
- fixed caller UID、paths、contest UID、template roots、unit allowlist；
- hardware collector、Home、logind methods；
- D-Bus policy/polkit；
- rustix mount/filesystem；
- invalid caller/parameter/fd/symlink tests；
- no external network or command shell。

### P6.3 Session Agent lifecycle

- graphical session registration；
- `session_instance_id`、session epoch、boot/logind mapping；
- Agent restart/replacement；
- Browser gate/start；
- local status/notification/binding prompt；
- no vault/password/Device/Gateway private key access。

### P6.4 Desktop-only LOCK/UNLOCK/TERMINATE

- exact SessionTarget；
- monotonic lock epoch；
- originating lock command ID；
- local journal before ACK；
- Agent full-screen gate + logind lock/unlock；
- same-session Agent/Daemon restart reasserts gate；
- reboot/new session invalidates stale unlock；
- instrumentation asserts zero Caddy Admin calls/config hash/process epoch changes。

### P6.5 Binding Prompt

- OPEN_BINDING_PROMPT typed command/expiry；
- seat code normalization；
- session target validity；
- BindingRequest/BindingResult；
- Server collision/manual/automation；
- binding creates target drift；
- optional auto SYNC_STATE；
- never auto SYNC_SECRET。

### P6.6 Browser policy

- fixed local origin；
- managed trust root/policy；
- single managed instance；
- start only after Gateway state allows；
- desktop lock hides/locks session but leaves Gateway running；
- restart/logout/termination behavior；
- no credential command line/environment。

### P6.7 Home template 与 backend

- versioned immutable template；
- manifest/mode/owner/xattr/ACL hash；
- OverlayFS lower/upper/work；
- target OS compatibility；
- staged-copy fixed argv fallback；
- deployment-time backend choice；
- instance GC。

### P6.8 Home Reset transaction

1. exact session/precondition check；
2. quiesce Browser/Agent/logind session；
3. create target instance + journal；
4. mount/copy staging；
5. verify；
6. detach old；
7. activate new；
8. verify active；
9. fsync commit；
10. resume；
11. async GC。

每个 durable step 后执行 kill/reboot；不确定状态必须 `manual_intervention_required` 并阻止 contest session。

### P6.9 Web/Operation UX

- Session exact target preview；
- stale session/lock diagnostics；
- lock/unlock/terminate operation progress；
- Binding prompt status；
- Home reset destructive confirmation、reason、re-auth；
- recovery guidance。

---

## 3. 实施顺序

### W25–W27

- D-Bus contracts/policy；
- Helper hardening；
- Agent registration/lifecycle。

### W28–W29

- lock/unlock/terminate state machine；
- stale/restart/reboot tests；
- zero-Caddy instrumentation。

### W30–W31

- Binding prompt/Browser policy；
- end-to-end binding → SYNC_STATE → SYNC_SECRET。

### W32–W33

- Home template/backends/reset transaction；
- kill/reboot matrix。

### W34

- target OS integration、operator UAT、G5 review。

---

## 4. 交付物

- final local-control D-Bus contract；
- hardened Privileged Helper；
- Session Agent；
- lock/unlock/terminate executors；
- Binding Prompt；
- managed Browser policy；
- Home template/backends/reset/recovery；
- Session/Home UI；
- runbooks；
- G5 evidence bundle。

---

## 5. 验证矩阵

| 场景 | 预期 |
|---|---|
| unauthorized UID calls Helper | D-Bus denied |
| arbitrary path/unit/string | schema/validator rejects |
| lock exact current session | desktop locked, Caddy unchanged |
| stale unlock old epoch/command | rejected |
| Agent restart while locked | gate reasserted |
| Daemon restart while locked | current session lock state recovered |
| reboot/new session | old unlock invalid |
| lock/unlock instrumentation | zero Caddy calls/hash/epoch change |
| Binding conflict | pending/rejected, no secret sync |
| Home crash at each step | recover or fail closed |
| busy old Home | no forced unsafe switch |
| staged-copy backend | fixed argv/paths only |
| Helper network attempt | impossible/blocked |

---

## 6. 风险与缓解

| 风险 | 缓解 |
|---|---|
| Desktop lock signal 不等于完成 | Agent gate acknowledgement 是最终完成事实 |
| stale unlock 解锁新 session | exact instance/epoch/lock epoch/command binding |
| lock 与 Caddy 耦合回归 | code ownership + zero-call integration test |
| Home reset busy mount | explicit recovery，不使用 lazy/force guess |
| D-Bus 接口膨胀为 remote shell | fixed typed methods/allowlists/security review |
| Browser policy差异 | 单一支持版本与 target OS package fixture |

---

## 7. G5 Gate 清单

- [ ] Helper no external network/arbitrary command；
- [ ] unauthorized UID denied；
- [ ] exact epoch-bound Session commands；
- [ ] stale unlock/new session tests；
- [ ] lock/unlock zero Caddy coupling；
- [ ] BindingRequest/BindingResult end-to-end；
- [ ] binding 不自动 secret sync；
- [ ] Browser managed launch/trust 通过；
- [ ] Home reset all kill/reboot points 通过；
- [ ] target OS full Client workflow 通过；
- [ ] G5 evidence 已签署。
