# 西安赛区 journald 日志分析

原始文件是 `context/natsume-xian.log`，由 `journalctl -o json` 导出，每行一个 JSON 对象，共 1,186,268 行、1,552,593,792 字节。`SYSLOG_IDENTIFIER=natsume-server` 的 `MESSAGE` 保存 tracing 文本；`systemd` 记录单元启动、停止和资源消耗。原始文件由 `.gitignore` 排除，脚本和测试中不保存真实日志行。

## 重现步骤

在仓库根目录执行，使用仓库指定版本的 Deno。脚本使用 Web Streams 和现有的 `@std/path`，不新增依赖。离线运行前执行 `deno task prepare scripts` 准备锁定的脚本依赖。

```sh
deno task prepare scripts
deno test --frozen --cached-only --no-prompt --allow-read=target/bench-analyze-tests --allow-write=target/bench-analyze-tests --allow-run=deno bench/analyze
/usr/bin/time -v deno run --frozen --cached-only --no-prompt --allow-read=context/natsume-xian.log --allow-write=context/bench/data/xian-2026-http bench/analyze/journald.ts extract context/natsume-xian.log --out context/bench/data/xian-2026-http
deno run --frozen --cached-only --no-prompt --allow-read=context/bench/data/xian-2026-http --allow-write=context/bench/data/xian-2026-http bench/analyze/journald.ts summarize context/bench/data/xian-2026-http --out context/bench/data/xian-2026-http/summary.json \
  --window warmup=2026-10-17T14:00:00+08:00/2026-10-17T17:00:00+08:00 \
  --window contest=2026-10-18T09:00:00+08:00/2026-10-18T14:00:00+08:00 \
  --analysis-end 2026-10-18T19:30:00+08:00
deno run --frozen --cached-only --no-prompt --allow-read=context/bench/data/xian-2026-http --allow-write=context/bench/data/xian-2026-http/series bench/analyze/journald.ts series context/bench/data/xian-2026-http --out context/bench/data/xian-2026-http/series \
  --series-range 2026-10-16T00:00:00+08:00/2026-10-19T00:00:00+08:00
deno run --frozen --cached-only --no-prompt --allow-read=context/natsume-xian.log,context/bench/data/xian-2026-http bench/analyze/check_anonymized.ts context/natsume-xian.log context/bench/data/xian-2026-http
deno task check scripts
```

`extract` 只顺序读取原始文件一次，逐行写入请求和其他事件，内存只保存进程及标识符别名表。输入必须按 `__REALTIME_TIMESTAMP` 非递减排列，否则立即失败。`summarize` 和 `series` 只读取匿名数据，时延样本使用 `Float64Array` 保存整数微秒；排序和汇总时的内存随样本及分组数量增长。各命令覆盖其负责的输出文件；失败退出后，应重跑该命令再使用结果。独立检查器保存原始敏感值集合，其内存随 journal 游标等唯一值数量增长。

## 时间与统计口径

数据中的时间戳均为 Unix 微秒，保留真实日期已获作者授权。命令行时间接受带 `Z` 或 `±HH:MM` 偏移的 ISO 8601 格式，如 `2030-01-02T09:00+08:00` 或 `2030-01-02T01:00:00.123456Z`；秒可省略，小数秒最多六位。输出统一使用 UTC+8。所有区间均为左闭右开，开始时间必须早于结束时间；无效时间、重复名称或选项错误会显示具体原因并以非零状态退出。

| 选项 | 使用规则 |
| --- | --- |
| `--window <name>=<start>/<end>` | 可重复，供 `summarize` 定义命名窗口；省略时只生成 `all` 和各进程窗口。名称须以英文字母开头，其余只能是英文字母、数字、`_` 或 `-`；名称区分大小写且不得重复。`all` 和 `p` 后接数字的进程别名形式为保留名称。 |
| `--analysis-end <iso>` | 可选，供 `summarize` 标记进程启动时间是否严格早于截止时间；等于截止时间时为 `false`。省略时 `analysis_cutoff` 为 `null`，进程表和重连数据的 `before_analysis_cutoff` 均为 `true`。该选项只设置标记，所有请求和进程仍参与统计。 |
| `--series-range <start>/<end>` | `series` 必填，仅限制 `minute_routes.csv` 的请求时间范围；重连曲线和时延分位点仍使用全量数据。 |

`series` 也接受并校验 `--window` 和 `--analysis-end`，以便共用分析参数；这两个选项只影响 `summarize` 的输出。`extract` 只接受输入路径和 `--out`，提取不依赖时间配置。

西安数据复现命令中的取值及来源如下：

| 名称 | 开始（UTC+8） | 结束（UTC+8） | 来源 |
| --- | --- | --- | --- |
| `warmup` | 2026-10-17 14:00 | 2026-10-17 17:00 | 根据流量推断的热身时段 |
| `contest` | 2026-10-18 09:00 | 2026-10-18 14:00 | 作者确认 |
| 图表时间段 | 2026-10-16 00:00 | 2026-10-19 00:00 | 图表展示 10 月 16 日至 18 日 |
| 赛后分析截止 | — | 2026-10-18 19:30 | 搬动服务器时断电的时间 |

`all` 和各进程窗口覆盖该组第一条到最后一条 HTTP 事件（末端加一微秒）。命名窗口采用命令行给定的区间，即使没有请求也保留整段时长。有效窗口及截止时间写入 `summary.json` 的 `named_windows` 和 `analysis_cutoff`。

completed、rejected 分开计数。请求份额的分母是同一窗口、同一事件类型的总数；route、method、status 的边际分组和三者的联合分组均输出。时延、每分钟及每小时流量、处理器耗时只计算 completed。每分钟校徽计数包含窗口内所有分钟，空分钟计零，边界分钟计该窗口内的请求；每小时结果只列出非零分组。请求率为 completed 数除以窗口秒数。

分位数采用最近秩：排序后取 `ceil(n × p / 100)` 对应的样本，最小索引为 1；空样本返回 `null`。时延单位为微秒。处理器耗时份额是每条路由 `duration_us` 之和除以该进程全部 completed 的时长之和；并发请求的时段可以重叠，这个量与 systemd CPU 时间含义不同。

重连曲线以 systemd Started 时间为零点，缺失时使用首条服务事件，统计前 600 秒 `/api/v2/device/control` 的 101 completed，每秒一个桶。延迟分位数只使用这 600 秒内的升级事件。进程中途结束后，后续桶仍为零；进程结束信息同时保留在进程表中。

## 输出文件结构

CSV 使用 UTF-8、CRLF 换行、首行列名，空字段表示缺失。JSON 的缺失数值使用 `null`，份额为 0 到 1 之间的小数。

### `requests.csv.gz`

每条 completed 或 rejected 一行。

| 列 | 含义 |
| --- | --- |
| `ts_us` | journal 的真实时间戳 |
| `process` | 按开始顺序分配的 `p01`、`p02` 等进程别名 |
| `level` | tracing 日志等级 |
| `event` | `completed` 或 `rejected` |
| `method` | HTTP 方法 |
| `route` | tracing 中的模板路由；保留公开 API 模板及 `<unknown>`、`<unmatched>`、`/` 等路由标记 |
| `status` | HTTP 状态码，未记录时为空 |
| `duration_us` | completed 的处理时长，rejected 未记录时为空 |
| `code`、`cause` | 机器可读错误类别，非类别文本删除 |
| `actor` | 按首次出现分配的 `operator-1`、`operator-2`；无操作员时为空 |

### `lifecycle.csv`

每个进程一行。`process` 为进程别名；`boot` 为首次出现顺序的 `b1`、`b2` 等启动别名。`start_ts_us` 为 systemd Started 时间，缺失时取服务第一条事件；`first_server_ts_us`、`last_server_ts_us` 为服务首尾事件；`stop_ts_us` 为 systemd Stopped、失败或服务 graceful shutdown completed 时间，缺失时留空。

`cpu_seconds`、`wall_seconds`、`memory_peak_bytes` 从对应进程的 systemd Consumed 行解析。周、天、时、分、秒按真实时长转换；K、M、G 等内存后缀按 1024 的幂换算，按最近偶数舍入到整数 bytes，精度受 systemd 原始显示小数位限制。没有 Consumed 行的进程保留空值。

`end_reason` 记录 `graceful`（服务完成优雅关闭）、`stopped`、`deactivated`、`failed`、`boot_changed`（后续观察到另一次系统启动）、`replaced_without_stop`（同一次启动中出现新进程但旧进程无结束记录）或 `log_end`（直到日志结束仍未观察到结束）。后面三种情况的停止时间保持空白。

### `notes.csv`

每条其他服务事件一行，列为 `ts_us,process,level,event,organization,port`。`event` 为 `server_mode_running`、`database_ready`、`server_state_ready`、`tls_identity_loaded`、`listener_bound`、`shutdown_initiated`、`shutdown_completed`、`logo_read_failed` 或 `other`。`organization` 只保留 `org-01` 等别名；`port` 只保留监听端口的十进制数。原始消息正文不输出。

### `summary.json`

| 字段 | 内容 |
| --- | --- |
| `timezone`、`window_convention`、`named_windows`、`analysis_cutoff` | 时区、区间规则、命令行指定窗口的 ISO 时间和截止时间（省略时为 `null`） |
| `windows` | 键为 `all`、命令行指定的窗口名称及进程别名 |
| `windows.*.bounds_ts_us`、`events`、`completed_requests_per_second` | 窗口边界、事件总数、completed 每秒平均数 |
| `windows.*.counts_by_route/method/status` | 每项包含 event、对应维度、count、share |
| `windows.*.requests` | 每项包含 event、route、method、status、count、share |
| `windows.*.latency_us` | 按 route、status 分组，含 count、p50、p95、p99、max |
| `windows.*.logo_requests_per_minute` | 公开校徽路由每分钟 completed 数的 count（分钟数）、p1、p50、p99、max |
| `windows.*.hourly_completed_requests` | 每项包含带 UTC+8 时区的 hour、route、count |
| `processes` | lifecycle 全部字段及 start_local、before_analysis_cutoff、handler_wall_time_us、handler_wall_time；后者每项为 route、duration_us、share |
| `reconnect_storms` | 每项含 process、before_analysis_cutoff、horizon_seconds、600 个 counts_per_second、delay_us（count、p50、p90、p99、max）、peak_per_second |

### `series/`

| 文件 | 列与内容 |
| --- | --- |
| `minute_routes.csv` | `minute,route,count`；`--series-range` 内的 completed 每分钟路由计数，仅列非零桶；图表补齐缺失桶为零 |
| `reconnect.csv` | `process,second,count`；每个进程固定 600 行，second 为从启动起的 0 至 599 秒 |
| `latency_cdf.csv` | `route,status,quantile,duration_us,sample_count`；全量日志的公开校徽 304 和 `/api/v2/devices` 各状态分别输出 0、0.01、…、1 共 101 个经验分位点，0 对应最小值 |

## 匿名规则与检查

进程使用 `(boot, PID)` 作为内部键；输出只写进程和启动别名，PID、journal 元数据和原始 UUID 全部丢弃。操作员 UUID、组织编号按首次出现顺序分配别名，保留同一数据集内的关联关系。重复出现的 tracing 字段要求值相同。`MESSAGE` 为整数 byte 数组时，先按 UTF-8 解码。

HTTP 输出使用固定列，只保留统计所需字段。路由接受公开 API 的静态路径和具名占位符，遇到不支持的路由中止提取，避免将具体资源标识写入数据。code、cause 只接受有限长度的 ASCII 类别形式。其他服务消息采用固定事件白名单，删除学校名、原始组织编号、路径、错误正文和监听主机，只保留组织别名及端口。未识别的服务消息统一输出 `other`。

独立检查器只与提取器共用通用文件读取函数，字段收集和匹配规则独立实现。检查器重新读取原始文件，收集 actor_id、主机、机器、启动、流、调用、序列及游标标识；收集文件路径、非 ASCII 字符、IP 地址和监听主机，再递归检查数据目录中的每个文件，`.gz` 会先解压。文件路径检查豁免公开的 `/api/` HTTP 路由。组织编号按原始 `organization_id=数字` 或带引号的字段表示匹配，因为同一短整数也可能合法出现在时延或计数中。游标及其分号分隔字段采用 token 集合查找，其余敏感值采用子串匹配。检查前还会展开 JSON 的 Unicode 转义和双反斜杠。检查失败只输出敏感值种类及命中次数，不输出原值。

## 已知限制与现场事实核对

日志提供 HTTP 层事件；设备控制连接内部状态、客户端行为及物理设备状态没有记录。101 表示 WebSocket upgrade；设备认证完成情况需要连接内部状态记录。作者提供的现场设备数量为 418 台，日志记录了各次重启后的升级次数。

真实数据有 10 个服务进程，开始和结束日期为 UTC+8 的 2026-10-01 至 2026-10-30。10 月 18 日 08:29:04 和 08:29:12 启动的进程，在启动后 600 秒内各有 418 次控制连接升级；19:30 后启动的进程保留在表中，标记为正文分析范围之外。

全量 completed 中，公开校徽路由所有状态合计占 98.5463%，其中 304 占全部 completed 的 98.3405%。比赛窗口的对应数值分别为 99.5203% 和 99.2247%。整个校徽路由占全部 completed 的份额，四舍五入到一位小数为 98.5%。

## 本次运行验证

10 项合成测试通过，涵盖重复字段、byte 数组消息、别名稳定性、生命周期、资源消耗单位、窗口边界、最近秩分位数、时间配置及缺省语义、非法参数和植入泄漏后的退出码。测试在 `target/bench-analyze-tests` 下生成虚构输入，并在结束时清理。

TypeScript 对真实输入完成提取、汇总和时序导出。解压后的 requests CSV、lifecycle CSV、notes CSV 和全部 series CSV 与已验证的 Python 输出逐字节一致；summary JSON 的结构和数值相同，浮点比较使用 1e-12 相对容差。使用上述显式时间参数重跑 `summarize` 和 `series` 后，与配置改动前的西安输出比较，summary JSON 严格深度相等且逐字节一致，三个 series CSV 逐字节一致。独立匿名检查器全量扫描通过。真实日志的日期范围、10 个进程、两次重启时间、每次 418 次升级、校徽路由及 304 份额和赛后进程标记均通过断言核对。

提取墙钟时间为 18.71 秒，峰值 RSS 为 152,468 KiB，退出状态为 0；运行环境为 Linux、Deno 2.9.8，输入已在文件缓存中。`deno task check scripts` 检查脚本类型、格式和 lint。数据和文章保留在由 Git 忽略的 `context/` 中。

执行 `prek run --all-files` 时，脚本、依赖策略、Web 格式、Web lint 和 Web 测试通过；Rust Clippy 和 Rust 测试因缺少有效的 `deno task prepare rust` 准备记录而未运行。
