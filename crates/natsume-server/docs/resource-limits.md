# 资源保护验证

本文说明测试与测量方法；限制归属和失败语义见[目标架构](../../../docs/architecture.md#153-desired-state-operator-api)和[Import 规则](../../../docs/architecture.md#152-import)。所有测试使用合成名单、临时数据库和回环连接。

## 常规回归

在仓库根目录执行：

```sh
cargo test -p natsume-server --lib component::import --locked
cargo test -p natsume-server --lib component::target_submission --locked
cargo test -p natsume-server --lib http:: --locked
cargo test -p natsume-server --lib tls:: --locked
cargo test -p natsume-server --lib device_control:: --locked
```

测试覆盖以下边界：

- JSON 入口分别覆盖恰好 2 MiB 和超出一字节的声明长度及流式请求；合法边界正文进入业务处理，超限沿用 JSON 提取失败的 400 错误映射。600 台设备的完整 Unicode 转义列表仍可提交。
- 2 MiB 工作簿可预览和提交；超过 2 MiB 的上传由 Bytes 提取器返回 413。Import 内部的 8 MiB 格式上限不扩大 HTTP 上传范围。
- Import 预览创建与提交共用容量；忙碌重试返回 503 和 Retry-After，不轮询请求体，也不阻塞模板下载和候选查询。
- 取消读取会释放名额；取消排队解析不会提前释放仍在阻塞执行器中的工作。取消事务调用方不撤销已开始的事务，成功与失败路径分别验证提交和回滚后的容量恢复。
- 共享字符串累计文本在 64 MiB 减一、恰好 64 MiB 和加一处验证；超过限制在复制前拒绝，错误不回显单元格内容。
- Target 保留 600 台有效设备的真实写入、回执重放、相同操作 ID 的并发去重及事务失败整批回滚。

## 可选的本机测量

资源探针默认忽略，不加入常规测试或持续集成容量门禁。每个解析样本使用独立进程，避免先前分配和样本生成污染进程峰值；Linux 的 VmHWM 记录整个测试进程的历史峰值，并不是精确的单次分配量。

```sh
set -eu
fixtures=$(mktemp -d)
trap 'rm -rf -- "$fixtures"' EXIT

NATSUME_RESOURCE_FIXTURES="$fixtures" \
  cargo test -p natsume-server --lib generate_workbook_resource_fixtures --locked -- --ignored --nocapture

for name in nominal maximum maximum-unique shared-amplification; do
  NATSUME_RESOURCE_WORKBOOK="$fixtures/$name.xlsx" \
    cargo test -p natsume-server --lib measure_workbook_resource_usage --locked -- --ignored --nocapture
done

cargo test -p natsume-server --lib measure_six_hundred_device_submission_cost --locked -- --ignored --nocapture

NATSUME_RESOURCE_WORKBOOK="$fixtures/maximum-unique.xlsx" \
  cargo test -p natsume-server --lib measure_import_and_target_burst_with_six_hundred_tls_control_connections --locked -- --ignored --nocapture
```

样本包括 600 行名单、10000 行最大字段名单、10000 行且学校各不相同的最大字段名单，以及使用重复共享字符串放大文本的小型工作簿。探针断言合法样本的队伍与学校数量，放大样本应以累计文本超限拒绝；密码全部为合成文本，不输出密码或原始单元格。

批量探针对显式设备、全部启用设备和模拟在线设备快照分别执行十次 600 台设备的实际更新，并核对回执重放不重复推进目标。

混合探针在一个临时 TLS Listener 上建立 600 条经过当前密钥证明和完整 ClientState 的 TLS 1.3／WSS 连接，持续发送心跳，同时进行完整名单预览与提交、十次并发批量 Target 更新和单设备状态查询。它检查连接持续有效并报告查询和心跳延迟；心跳统计在全部连接建立后清空，只覆盖混合负载阶段。状态查询持续到全部操作结束，保留最后一次在途查询的完整延迟，不通过取消慢查询截断统计。探针使用四个异步执行线程，服务端和所有模拟客户端在同一个进程，进程峰值包含两者。模拟客户端发送初始 ClientState、持续心跳并接收目标，不执行工位动作，也不模拟动作完成后的 Actual 更新。名单、Target 操作和状态查询通过进程内组件入口发起；其计时不包含 HTTP 认证、请求传输或大型 JSON 预览响应序列化。

## 验收边界与已知限制

容量许可限制并发工作数量；调用方取消不会自动停止后台解析或撤销已开始的数据库事务，进程总内存仍须独立控制。若后续需要严格的计算时间或总内存隔离，应在部署基线和测量明确后独立设计。

当前 Operator API 没有统一请求处理期限，也不对请求体读取独立计时；慢速上传可长期占用 Import 名额，直到请求终止。JSON 和 Bytes 提取器使用默认的 2 MiB 原始正文上限，解析对象和并发请求总内存须另行测量。

当前服务入口没有启用 Hyper 的 HTTP/1 请求头计时器。修复服务入口及其 TLS、WebSocket 升级和优雅关闭回归属于独立工作。
