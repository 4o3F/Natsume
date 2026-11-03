# Natsume loadgen

`natsume-loadgen` 通过真实 TLS 1.3、HTTPS 和 WSS 模拟工作站与 Operator 请求，使用共享协议 crate 的 protobuf 和 ClientProof 签名。它是 workspace member，未加入 default-members 或产品打包清单。所有生成的学校、队伍、账号和密码均为合成数据；setup 会替换服务器名单，目标场景覆盖服务器全部 Enabled 设备，因此应使用独立测试服务器和数据库。

## 准备服务器与名单

先按 [部署手册](../../docs/operations-deployment.zh-CN.md) 配置并启动测试服务器。HEAD 和 v2.10.3 的初始管理员均通过 `natsume-server bootstrap` 在控制终端提示输入登录名、密码和确认密码；两版均没有初始管理员的非交互 CLI 或公开 HTTP 创建入口。这是首次部署需要手动执行的一步，之后 setup 使用现有管理员登录。

```sh
natsume-server bootstrap
natsume-server serve
```

在另一个终端创建只允许当前用户读取的密码文件，例如 `umask 077; read -rs operator_password; printf '%s' "$operator_password" > ./operator-password; unset operator_password`。密码文件允许末尾一个换行，不把密码传入命令行或 run.json。服务器证书必须由 `--trust-root` 指定的 CA 签发；所有请求均验证证书，不使用系统代理，不跟随重定向。

```sh
cargo build -p natsume-loadgen
./target/debug/natsume-loadgen setup \
  --server https://127.0.0.1:8443 --trust-root ./control-ca.crt \
  --operator-login synthetic-admin --operator-password-file ./operator-password \
  --teams 100 --schools 34 \
  --logo-dimension 256 --logo-bytes 120832 \
  --logo-directory /srv/natsume-test/organization-logos --output-dir ./fixture
```

setup 依次登录、生成合成 WebP、生成 XLSX 与座位 CSV、预览并提交名单、打开自动审批窗口、检查组织数量和全部 Logo 的 HTTPS 可读性。省略 `--schools` 时使用 `ceil(teams / 3)`。`--output-dir` 必须是新目录；现有同名合成 Logo 不会被覆盖，失败后检查已完成步骤再选择新的测试目录。输出包含 `roster.xlsx` 和单列 `seats.csv`，账号格式为 `synthetic00000`、座位为 `S00000`。

两版均没有 Logo 上传 HTTP 接口。`--logo-directory` 必须对应 Server 的 `storage.organization_logos`，远程服务器可通过已挂载的目录安排文件；Server 用户需要读取权限。文件按 `Synthetic School 00000.webp` 命名，权限 0644。默认 256×256、120832 字节（118 KiB），支持 16–2048 的方形尺寸和 1 KiB–8 MiB 的偶数字节数；编码图像加上 RIFF chunk header 必须能装入所选字节预算。图像使用现有 workspace `image` 的 WebP 无损编码器，追加合法 RIFF `JUNK` chunk 得到精确传输大小。这个参数模拟现场全部 WebP、约 1033 份学校 Logo、平均约 118 KB 的流量配置；如需 1033 个学校，可设置 `--teams 3099 --schools 1033`，运行时仍需考虑服务器连接上限。

也可让 setup 与 run 读取同一个 `--scenario`；setup 从中读取 `devices`、`schools`、`logo_bytes` 和 `logo_dimension`。场景中给出的值覆盖同名用途的命令行值，未给出的值使用命令行或默认值。

## 运行与持久化

```sh
./target/debug/natsume-loadgen run \
  --server https://127.0.0.1:8443 --trust-root ./control-ca.crt \
  --state-dir ./fleet --seat-plan ./fixture/seats.csv --output-dir ./run-001 \
  --operator-login synthetic-admin --operator-password-file ./operator-password \
  --scenario crates/natsume-loadgen/scenarios/S3-contest.toml
```

`--seat-plan` 是可选 `seat_code` 表头的单列 CSV，支持引号和转义引号，保留座位文本与前导零，第 i 行分配给第 i 台设备，座位不得重复。`--state-dir` 使用进程锁，包含测试私钥，需限制访问。复用同一 state-dir、seed、设备序号与座位分配时执行 Resume；每次使用新的 output-dir。硬件 UUID、控制测试密钥与随机调度由 seed 推导，Gateway P-256 密钥来自系统随机源。Ctrl+C 取消任务并完成 gzip 写入。

`--arrival uniform --ramp 60s` 把第 i 台设备的循环启动安排在 `i × ramp / (devices−1)`；启动后还执行默认 0–5 秒的 daemon 首次连接随机等待。因此最后一次首连可能晚于 ramp。`--arrival all-at-once` 同时启动设备循环，要求 ramp 为零，仍保留首次随机等待。duration 从最后一个计划启动时间开始计算，包含握手、重连和 Operator 工作；场景事件时间从整个 run 开始计算。时长使用整数加 `ms`、`s`、`m`、`h`，上限 7 天。

资源延迟均匀分布在 `[resource-delay, resource-delay + resource-jitter]`，默认 20 ms、零抖动。`gateway-delay`、`binding-delay`、`runtime-delay`、`session-delay`、`home-delay`、`power-delay` 可分别覆盖基础延迟。`logo-interval` 默认 60 秒，零关闭。退避和心跳默认值与 daemon 相同：首次及重试随机窗口 5、5、10、20、30 秒；稳定 Active 至少 60 秒后收到合法状态或匹配 Pong 才重置窗口；Ping 周期 30 秒，静默期限 60 秒。可用 `reconnect-initial`、`reconnect-max`、`stable-after`、`ping-interval`、`silence-timeout` 覆盖。

设备持久化 identity、authority、Gateway key/CSR/leaf、Binding input、资源完成 epoch；enrollment_ready 和 Actual 在对应持久化完成后发送。重复 Target 保留资源原有执行期限，新 Target 替换未执行工作。Binding 等待 Home、contest 会话和 waiting 前台就绪，再模拟一次志愿者确认。Gateway 使用真实 P-256 CSR，核对签发 leaf 的公钥，回报完整 DER 的 SHA-256。Session 前台跟随 Target 的 waiting/contest；凭据、Runtime、Home、Session 满足条件后报告 Gateway Ready。断线撤销模拟访问权限，重连先发送完整本地快照。

每台设备有独立 Logo HTTP 连接池。[daemon 控制连接](../natsume-workstation-daemon/src/control/connection.rs) 在结束 Active 时调用 `presentation_offline`，由 [presentation](../natsume-workstation-daemon/src/reconcile/presentation.rs) 的 `disconnect` 标记离线，`download` 据此暂停下载并保留 ETag；[snapshots deactivate](../natsume-workstation-daemon/src/reconcile/access.rs) 另外撤销访问权限。loadgen 在断线时暂停下载并保留 ETag；新完整 Target 恢复组织选择后立即条件请求。每组织 ETag 写入 state-dir 的 `logos-00000.json` 等文件，绑定服务器 origin 和 CA 摘要，组织切换、控制重连和 loadgen 重启均可复用。404 删除该组织的 ETag，响应失败保留原值。

## Operator 与场景

`run --operators K` 增加 K 个独立面板会话，各自登录并拥有 HTTP/TLS 池。每个会话每 `--poll-interval`（默认 10 秒）依次读取 `/devices`、`/bindings`、`/seats`、`/accounts`、`/organizations`、`/provisioning-window`、`/enrollment-reviews`，每 `--session-interval`（默认 30 秒）读取 `/session`；路径均位于 `/api/v2`。GET 收到 401 后重新登录并重试一次，普通轮询失败记录后继续。指定定时事件时增加一个只执行计划的 Operator 会话，K 仍为轮询面板数量。

场景是最小 TOML：顶层可配置 `devices`、`arrival`、`ramp`、`duration`、`operators`、`poll_interval`、`session_interval`、`logo_interval`、`schools`、`logo_bytes`、`logo_dimension`，以及上述资源延迟、退避和心跳参数（横线改为下划线）。未知字段和乱序事件会被拒绝。场景值覆盖命令行值，修改副本即可改变内置场景的规模或时间。

```toml
devices = 100
ramp = "10s"
duration = "10m"
operators = 2
logo_interval = "60s"
reconnect_max = "30s"
[[events]]
at = "300s"
action = "contest"
[[events]]
at = "360s"
action = "import"
roster = "changed-roster.xlsx"
```

`events` 按 at 非递减排列，时间必须早于 run 结束。action 为 `waiting`、`contest`、`terminate`、`reset_home`、`export`、`import`。目标动作向 `/target-submissions` 提交新的 UUID operation_id、`all_enabled` scope 和对应 action；导出读取 `/exports/domjudge`；导入调用 `/imports` 预览，再携带 preview token 与相同 XLSX 调用 commit。事件中的 roster 路径相对场景文件；未指定时使用 `--operator-roster`（相对工作目录）。重复导入 setup 生成的名单可测预览／提交负载，修改队伍展示字段可测有变化的导入，已有绑定的座位和账号映射须满足 Server 的导入约束。写请求结果不明或有设备拒绝时结束运行并保留记录。

| 场景 | 配置与操作 |
| --- | --- |
| [S1 cold start](scenarios/S1-cold-start.toml) | 600 台，60 秒渐进接入，保持 5 分钟；使用新的 fleet state |
| [S2 steady](scenarios/S2-steady.toml) | 600 台，60 秒渐进接入，保持 30 分钟 |
| [S3 contest](scenarios/S3-contest.toml) | 100 台、2 个面板；T+45 秒 contest、T+70 秒 terminate、T+95 秒 home reset |
| [S4 terminate/home](scenarios/S4-terminate-home.toml) | 600 台，T+120 秒 terminate，T+180 秒 home reset |
| [S5 reconnect](scenarios/S5-reconnect.toml) | 600 台，在 T+300 秒从外部重启测试 Server，保持 loadgen 进程与 state-dir；核对每台 RESUME 后第一条 Logo 状态为 304 |
| [S6 import](scenarios/S6-import.toml) | 600 台、2 个面板；T+300 秒导入，T+360 秒导出，传入 `--operator-roster ./fixture/roster.xlsx` |
| [S7 panel tabs](scenarios/S7-panel-tabs.toml) | 600 台、5 个面板，可在副本中配置 K 和轮询周期 |

S3 从 waiting 状态开始。若复用已处于 contest 的 fleet，应先用 waiting 事件恢复。Server 重启会关闭进程内 Enrollment 自动审批窗口；已登记设备 Resume 不受影响，需要新设备接入时再打开窗口。

## CSV 与收敛测量

`events.csv.gz` 使用表头 `ts_us,device,event_kind,latency_us,detail_code`。ts_us 为记录写入队列时的 Unix 微秒；device 为从零开始的设备序号，Operator 事件使用面板序号，计划驱动使用零，依 event_kind 区分。普通 latency_us 是请求开始到完整响应读取／协议步骤完成的微秒数。detail_code 只写固定诊断、资源名、HTTP 状态或操作标识，不写响应正文、Cookie、口令、CSR 或私钥。

| event_kind | latency_us 与 detail_code |
| --- | --- |
| `tcp_tls`, `wss_upgrade`, `handshake` | TCP/TLS、WSS upgrade、应用握手；状态或 ENROLLMENT/RESUME |
| `first_snapshot`, `first_target`, `target_actual`, `ping_rtt` | 首次快照发送、首次 Target、资源变化到 Actual 发送、匹配 Pong；资源变化也包含协商 Input |
| `logo`, `disconnect`, `reconnect_delay` | Logo 完整 HTTP 响应、断线原因、计划随机等待；HTTP 状态或固定代码 |
| `operator_login`, `operator_session`, `operator_devices`, `operator_bindings`, `operator_seats`, `operator_accounts`, `operator_organizations`, `operator_provisioning`, `operator_enrollment`, `operator_export`, `operator_import_preview`, `operator_import_commit`, `operator_target_submission` | 每个 HTTP 请求的完整读取耗时和状态，包括失败请求 |
| `operator_submission_receipt` | `operation_id:ACTION:submitted:rejected`，latency 为零 |
| `convergence_target`, `convergence_actual` | 从提交响应完整读取到设备收到匹配 Target／发送匹配 Actual 的有符号微秒数；`operation_id:ACTION` |

提交前读取设备基线并启动观测，响应返回后仅保留 receipt 中提交成功的设备。foreground 按 waiting/contest 匹配，terminate 和 home reset 按基线 epoch 的后继匹配，Actual 在完整快照发送完成后计时。设备可能早于 Operator 收到响应，因此这些延迟允许负数；此时 ts_us 是延后入队时间，latency_us 仍保留原始观测与响应的时间差。已经等于所请求 foreground 的设备不计入本次变更样本。后续同资源动作会停止先前动作未完成的观测，run.json 保留各操作的期望数、Target 数、Actual 数和 superseded 标记。测量期间由这一计划独占目标写入，以保持基线 epoch 与提交的对应关系。

按 operation_id 分组计算每台设备的 Target／Actual 延迟 p50、p99，最大 Actual 延迟为该次观测到的全量设备完成时间；先核对实际样本数与 expected 一致。`run.json` 保存生效参数、事件计划、收敛计数、events 与 dropped_events，seed 仅保存在私有 fleet state。事件写入采用有界队列的 try_send，队列满时计入 dropped_events，设备循环继续；最终 tracing 摘要也报告丢弃数。dropped_events 非零时 CSV 可能缺少收敛样本。writer 故障作为带 source 的 Snafu 错误结束运行，fatal 日志通过 `snafu::Report` 输出错误链。

## 与真实 daemon 的差异

Session、Home、Power、Caddy 和凭据安装由本地状态模拟，资源延迟可配置；Power Accepted 后设备任务继续。Logo 只读取有上限的完整 HTTP 响应并保存 ETag，不解码、不缓存图像内容；每组织缓存会保留多个学校，daemon 只保留当前展示绑定的图片。合成 WebP 的 padding 控制流量大小，其图像解码工作量与现场图片不同。Operator 会话按配置轮询全部上述列表，使用串行请求，导出完整读入内存后丢弃，忽略浏览器渲染与缓存；定时事件等待上一次 HTTP 响应后继续执行。已完成资源 epoch 单调保存，完整实际桌面故障与物理输出不在模拟范围内。

Server 两版 TLS listener 均固定最多 1024 条连接，WSS、Logo HTTP 和 Operator HTTP 共用预算；扩大设备数前需要考虑这一限制。负载生成器的同机 CPU、磁盘持久化和调度也会进入本地耗时。

## 两版兼容与本地验证

对比对象为 HEAD `ed78c90` 与 v2.10.3 tag `8a74e5e`（指向 commit `8d5f5d0`）。对照 HEAD 的 `crates/natsume-server/src/http/handler/` 与 v2.10.3 的 `server/src/http/handler/`：session、target_submission、import、contest/organization、provisioning、device/lifecycle 的 HTTP 请求字段、Cookie 名称、preview token 与目标 receipt 结构保持兼容。

| 差异 | loadgen 处理 |
| --- | --- |
| HEAD 增加 `state=non_revoked` 等设备筛选；v2.10.3 支持逗号分隔生命周期 | 请求不带 state 参数的 `/devices`，按返回的 state 识别 Enabled |
| HEAD Import 使用默认 2 MiB HTTP 正文限制；v2.10.3 为 8 MiB | 预览前检查 XLSX 不超过两版共有的 2 MiB 预算 |
| v2.10.3 登录正文限制 8 KiB、读取超时 5 秒；HEAD 使用默认提取限制 | 发送完整、小型 UTF-8 JSON；共同使用客户端请求期限 |
| HEAD 增加 journal、审计、停机协调；handler 内部状态拆分 | 使用共有业务路由和返回字段，不轮询仅 HEAD 存在的 journal 路由 |

验证命令：

```sh
cargo clippy -p natsume-loadgen --all-targets -- -D warnings
cargo test -p natsume-loadgen
cargo deny check
deno task check scripts
deno task prepare rust
prek run --all-files
```

2026-11-03 本机验收使用独立临时数据库、测试 CA 和合成管理员。v2.10.3 在仓库树外的 detached worktree 构建，验收后已移除该 worktree。每版通过 Rust setup 导入 100 个队伍／座位和 34 所学校，WebP 均为 256×256、120832 字节，再执行内置 S3：100 台设备、2 个轮询面板，默认 20 ms 资源延迟。绑定与 contest、terminate、home reset 均为 100/100 收敛；每版 CSV 各包含 300 条 convergence_target 和 300 条 convergence_actual，dropped_events 为零。

下表单位为毫秒，分位数在每次提交的 100 台设备上计算，起点为 Operator 完整读取提交响应；最大 Actual 延迟为本次观测的全量收敛时间。

| Server | 动作 | Target p50 | Target p99 | Actual p50 | Actual p99 | Actual 最大值 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| HEAD ed78c90 | contest | 33.162 | 40.692 | 94.165 | 133.645 | 136.344 |
| HEAD ed78c90 | terminate | 28.738 | 37.487 | 90.348 | 140.526 | 142.294 |
| HEAD ed78c90 | home reset | 27.716 | 31.388 | 89.830 | 126.552 | 127.770 |
| v2.10.3 | contest | 59.899 | 134.902 | 117.205 | 159.459 | 159.500 |
| v2.10.3 | terminate | 47.352 | 59.976 | 109.683 | 150.534 | 150.655 |
| v2.10.3 | home reset | 61.171 | 88.419 | 122.457 | 184.915 | 188.009 |

每版在 S3 后期重启本地 Server，100/100 Resume 并重新收敛，逐设备检查 Resume 后第一条 Logo 记录均为 304，零 200；随后保留 fleet state、重启 loadgen，100/100 首次 Logo 也为 304。HEAD 本轮初始与重启运行的 journal 共记录 20 次既有 SQLite 事务告警，内容为 Cannot perform this operation outside of a transaction；v2.10.3 未出现该告警。

两版另执行 100 台设备、2 个面板的 30 秒补充场景：导入预览返回 201、提交返回 204、DOMjudge 导出返回 200；waiting 和 contest 均记录 100/100 Target 与 Actual，丢弃数为零。

crate Clippy、15 项单元测试、cargo-deny、Deno 脚本检查和完整 prek 已通过。

## 按路由校准面板与实验编排

场景可设置 `[panel_rates]`，键为 `/devices`、`/session` 等上述 GET 路由，值为所有 `operators` 面板合计的每分钟请求数。每个面板使用独立 HTTP/1.1 连接池，按路由周期串行请求；面板间按周期等距错开，未指定或零频率的路由停用。存在此表时替代 `poll_interval` 和 `session_interval`；没有此表时保留原有默认频率。每次请求及失败继续进入 events CSV，run.json 保存生效的路由频率。

`probe` 子命令接收 server、trust-root、operator 凭据文件和 output-dir，每隔 10 秒重新建立一个 operator 会话，共六次；记录登录与 devices 请求的时延和结果，用于观察 TLS 连接上限处的新连接准入。HTTPS 超时与连接错误分别记录 `HTTPS_TIMEOUT`、`HTTPS_CONNECT`。

`generate-logos` 按聚合 profile 生成不同尺寸和字节预算的合成 WebP，支持带种子的图案细节、噪声分布和透明度纹理；`decode-logos` 测量逐文件与整组解码耗时；`setup --reuse-logos` 使用现有合成文件导入名单并检查公共 logo 路由。双机编排、资源采样、PTY bootstrap、CPU 火焰图和实验 TOML 见 [bench/run](../../bench/run/README.md)。

`run --start-at-unix-us EPOCH` 在身份准备完成后等待指定的 Unix 微秒，再开始设备到达、持续时间和计划事件计时；起点已过或超过七天时退出。编排器预留准备时间，使用此选项对齐两机的重启和导出计划。TCP reset 与 TLS EOF 分别记录为 `TCP_RESET`、`TLS_EOF`。

`export --at-unix-us EPOCH` 使用与 probe 相同的 server、trust-root、operator 凭据文件和 output-dir 参数，先登录，再在指定 Unix 微秒发起一次 DOMjudge 导出，将登录与完整导出的时延写入 events CSV。编排器在操作前重新测量时差，独立安排导出请求，避免 fleet 身份准备或 SSH 建连时间改变注入点。
