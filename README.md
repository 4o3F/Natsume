# Natsume V2

Natsume 是面向单场竞赛现场的工作站控制与访问编排系统。

全系统只有一份人工维护的目标架构与实施计划：

- [`docs/architecture.md`](docs/architecture.md)

该文档定义产品范围、进程边界、Device Control 状态模型、Server/Client 组件、目标数据库、安全约束、验证矩阵和 flag-day 实施顺序。当前代码仍可能落后于目标架构；实现现状不能反向定义目标。

## 仓库拓扑

```text
server/                  natsume-server
client/
  device-daemon/         natsume-device-daemon
  privileged-helper/     natsume-privileged-helper
  session-agent/         natsume-session-agent
crates/
  device-protocol/
  local-control-api/
  machine-identity/
web/                     Operator Web Panel
packaging/
docs/architecture.md     唯一架构文档
```

## 常用命令

```bash
just toolchain
just install
just fmt
just lint
just unit
just api
just verify
just package
```

精确依赖版本、生成契约和打包输入分别由 lockfile、机器 schema 和 packaging 配置拥有。
