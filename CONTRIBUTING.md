# 贡献指南

本文件收录**评审启发式**——用于判断设计是否在退化的问题清单。它们不是可测试契约，因此不属于 `docs/` 下的规范文档。

可测试的规则在别处：安全不变量见 [`docs/security-recovery.md`](docs/security-recovery.md) 的 `INV-*`，边界契约见 [`docs/contracts.md`](docs/contracts.md)，数据所有权见 [`docs/architecture.md`](docs/architecture.md)。那些是必须满足的；本文件是应当思考的。

## 常用命令

```bash
just verify      # toolchain + install + fmt + lint + unit + api + diagrams
just lint        # clippy + cargo deny + web lint/typecheck
just unit        # cargo test + web test
just integration # 跨 crate/协议/策略测试
just package     # 构建 Server/Client Deb
```

文档变更另外执行：

```bash
node docs/verification/validate-links.mjs docs README.md
node docs/verification/validate-markdown.mjs docs README.md
pnpm diagrams
```

## 设计变更自检

每个设计变更应回答：

1. 该规则是否只有一个变化原因和一个 owner？
2. 是否把 transport、database 或 OS 细节泄漏到 domain？
3. 是否要求多个模块同步修改同一事实？
4. 是否把少数平台特例放进核心状态机？
5. 是否创建了通用 manager、context、helper 或 error code 分支？
6. 是否把异步 Operation 模型强加给普通 CRUD？
7. 是否可以用 value object、capability 或 port 收敛边界？
8. 是否有负向测试证明禁止路径不能绕过？

### 应当拒绝的信号

- 一个 handler 同时操作 vault、SQL、Caddy 和 D-Bus；
- 一个共享 crate 只有一个消费者；
- 一个"全局状态"同时表达证书、配置、秘密和 session；
- 一个 UI 文案变化要求修改领域逻辑；
- 一个桌面环境差异要求改 wire protocol；
- 一个新错误必须让所有领域模块依赖全局 registry。

## 新模块自检

- Owner 和变化原因是什么？
- 哪些表、文件、secret 属于它？
- 它对外暴露的 port 是什么？
- 是否只有一个消费者而不应成为独立 crate？
- 是否泄漏 framework types？
- 是否需要 root 或外网？
- 是否能用现有 typed contract？
- 是否造成循环依赖？
- 是否可以独立测试？
- 删除它时影响哪些组件？

回答不清楚时，不新增 "manager/service/common" 层。共享 crate 的硬性准入条件见 [`docs/repository-layout.md`](docs/repository-layout.md) §4。

## 安全变更自检

涉及认证、授权、加密、输入校验或秘密管理的变更，额外回答：

- 资产和攻击者是谁？
- 新增了哪个 trust boundary？
- 哪个进程获得了新 capability？
- 是否能用更窄的 typed contract？
- secret 在哪里出现、保存多久、如何清零？
- identity/revision/epoch 如何绑定？
- 失败时保留什么、拒绝什么？
- 重试是否幂等？
- audit 是否原子？
- 正向、负向、故障注入和恢复证据在哪里？
- 是否需要更新 `INV-*`、ADR 或 Gate？

**没有 evidence locator 的安全声明不得用于 Gate PASS。**

## 何时需要 ADR

需要：产品范围与信任边界变化、新进程/新 root capability/新外部网络路径、身份与证书与秘密与 fail-closed 规则、wire compatibility 或持久化身份语义、新共享 crate、Home backend 策略、目标桌面启动模型、放宽任何 `INV-*`。

不需要：不改变稳定语义的内部重构、新增测试或 evidence、已定义 contract 内的新 adapter、不影响跨模块边界的性能优化。

架构变更合并时同步检查：对应规范文档、ADR、machine schema/golden、Gate 引用、安全与负向测试。

## 提交约定

使用 conventional commits（`docs:`、`feat:`、`fix:`、`chore:` 等），scope 用组件名，例如 `docs(architecture):`、`feat(contracts):`。
