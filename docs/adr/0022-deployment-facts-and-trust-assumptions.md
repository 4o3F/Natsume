# ADR-0022: Deployment facts and trust assumptions

> Status: `ACCEPTED`  
> Scope: Natsume V2  
> Supersedes: —  
> Superseded by: —

## Context

[ADR-0021](0021-provisioning-window-certificate-issuance.md) 证明了一个方法：先记录部署事实，机制才有校准依据。V2 早期规范按"更大威胁、更大团队、更长生命周期"隐式校准，产生了与实际部署不匹配的机制规格。本 ADR 把全部已确认的部署事实与信任假设固化为可引用条目；后续 ADR 与规范的裁剪均以此为依据。

## Decision

以下事实作为 V2 设计的校准基线。任何一条失效都必须先修订本 ADR，再调整依赖它的机制。

### 环境与规模

- **F1**：约 500 台受管工作站，**异构硬件**（非统一采购；v1 曾发生 MAC 地址冲突）。
- **F2**：单机房有线 LAN；网络由第三方管理；**带宽受限，必须节约**；UDP 通过性无保证。
- **F3**：单 Server 实例，**Server 有固定 IP**；工作站为 DHCP 短租期，**无法保证静态 IP 或长租期**。
- **F4**：基础 OS 镜像派生自 **ICPC 官方镜像**，大版本更新可能改变桌面栈；当前周期为 X11。最终镜像由本项目构建。
- **F5**：DOMjudge 为外部竞赛系统；其 web server 已启用 brotli；实测 brotli 相对 gzip 的带宽节约显著（保留本机 HTTPS 的依据）。

### 生命周期与团队

- **F6**：一次部署服务一场竞赛（见 [ADR-0009](0009-single-lifetime-minimal-domain.md)）；**产品跨赛事长期复用**。
- **F7**：开发窗口约 6 个月，团队 3 人。
- **F8**：操作员 1–3 人，互相信任；**不存在并发导入场景**；权限只需 admin 与 viewer 两级。
- **F9**：审计记录只面向赛事管理员，不对外提交。
- **F10**：Home 在热身赛后与赛前连续测试中**多次重置**；初始 Home 不能烤入镜像。

### 信任假设

- **T1**：赛前存在物理受控的 **provisioning 窗口**（继承自 ADR-0021）；窗口内操作员受信，窗口关闭后本机使用者（选手）不受信。
- **T2**：选手是**非 root** 本地用户；本地 root、物理攻击、固件篡改不在防护范围（与 [security-recovery.md](../security-recovery.md) §1 一致）。
- **T3**：选手不知晓自己的 DOMjudge 凭据；登录必须由系统代为完成。
- **T4**：venue 网络上可能出现未授权设备；**跨网线的流量视为可嗅探**，Server 与 DOMjudge 的身份必须密码学可验证。

## Alternatives

- **不固化事实、按最坏情况设计**：产生与 F1–F10 不匹配的机制规格（五段证书阶梯、双桌面矩阵、通用并发导入），已被逐项裁剪。
- **把事实分散写进各规范**：不可引用、不可整体失效检查。

## Consequences

### Positive

- 每个安全/复杂度决策都有可指认的事实依据；
- 事实变化时有单一修订入口，可反推受影响机制。

### Negative / trade-offs

- 部署形态变化（多场地、统一采购、团队扩张）时本 ADR 必须先行修订，存在维护纪律成本。

## Evidence and revisit trigger

- F5 的 brotli 收益来自 v1 实测；F1 的 MAC 冲突来自 v1 事故记录。
- 重开条件：多场地/跨互联网部署、镜像不再自建、操作员不再互信、选手获得 root、或带宽约束消失。任何一条触发时，依赖该事实的 ADR（0023–0029）必须重新评审。

## References

- [ADR-0009](0009-single-lifetime-minimal-domain.md)
- [ADR-0021](0021-provisioning-window-certificate-issuance.md)
- [architecture.md](../architecture.md)
- [security-recovery.md](../security-recovery.md)
