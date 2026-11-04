# 双机实验编排

`run.ts` 从工作站分别通过 SSH 控制 配置中的 Server 主机 A 和 loadgen 主机 B。A 运行隔离的 `natsume-bench.service`，B 在 NUMA node 0 上运行 loadgen。工具使用已有的隔离安装、基线快照、公有 CA、合成名单和 B 上的 `operator-password`。所有 SSH 调用启用严格主机密钥检查并关闭 agent forwarding。生产单元须为 disabled 且 MainPID 为 0。

## 执行

在仓库根目录运行：

```sh
deno task prepare scripts
cargo build --release -p natsume-loadgen
LOADGEN_HOST=$(deno eval --frozen --cached-only 'import { configuration } from "./bench/run/config.ts"; console.log(configuration(await Deno.readTextFile(Deno.args[0])).hosts.loadgen)' bench/run/experiments/B0-short.toml)
scp -o BatchMode=yes -o StrictHostKeyChecking=yes -o UpdateHostKeys=no -o ForwardAgent=no target/release/natsume-loadgen "$LOADGEN_HOST:/home/icpc/natsume-bench/B0/natsume-loadgen"
deno run --frozen --cached-only --no-prompt --allow-read --allow-write=context/bench/data/results --allow-run --deny-net bench/run/run.ts bench/run/experiments/B0-short.toml
```

每个实验使用新的本地结果目录和 B 上的 fleet 副本；目录已存在时退出。恢复程序检查快照摘要，将原有 bench data 移入同一文件系统上的 root-only staging，再发布快照中的 data。原有 data 保留在该 root-only staging 中。工具读取基线快照，不访问生产备份。

`version` 同时接受发布版本和构建变体标签，例如 `v2.10.3`、`v2-60fbe1a`、`pr17-956b1b8`。标签须以 ASCII 字母或数字开头，其余字符只允许字母、数字、点和横线。二进制位于 `/opt/natsume-bench/<version>/natsume-server`，配置和数据位于 `/srv/natsume-bench/<version>/`；运行前须将 `natsume-bench.service` 的 ExecStart、配置绑定和可写数据目录指向对应标签，工具检查 ExecStart 后恢复该标签的快照。

TOML 的 `name`、`version`、`devices`、`scenario` 标记实验；`ramp_seconds`、`warmup_seconds`、`steady_seconds` 定义时间窗口，`sample_seconds` 定义资源采样周期。`tabs` 为面板数量，`panel_rates` 为所有面板合计的各 GET 路由每分钟次数；省略的路由停用。`logo_interval_seconds=0` 关闭 logo。`baseline`、`fleet_baseline`、`seats` 分别相对 A 的版本目录和 B 的 B0 输入目录。`cold_etag=true` 清空当次副本的 logo 缓存；`require_active=true` 要求测量前全部设备已完成应用握手。

每份实验配置必须提供 `[hosts]`，`server` 和 `loadgen` 使用 `user@hostname` 格式且须不同，`server_url` 为不含凭据、路径、查询参数或片段的 HTTPS origin；端口由 URL 指定，省略时为 443。SSH 地址和 Server URL 均无代码默认值，证书须覆盖 URL 主机名。运行、容量准备和 bootstrap 都读取同一份配置。采样器使用 URL 的端口；资源 CSV 为兼容历史结果保留 `tcp_8443_established` 列名，该列记录配置端口的连接数。

```toml
[hosts]
server = "icpc@10.12.13.166"
loadgen = "icpc@10.12.13.20"
server_url = "https://10.12.13.166:8443"
```

`actions` 的 `at` 相对测量窗口开始计秒，支持 `restart`、`export`、`perf` 和 `probe`。restart 由 A 上提前安装的定时脚本执行，actions.csv 记录实际起止；loadgen 在身份准备完成后等待绝对起点；export 使用独立的预登录 operator，在 T 前重新校准时钟后等待指定的请求时刻；perf 额外使用 `seconds` 和可选 `call_graph`，默认用帧指针以 99 Hz 采样指定 PID；probe 在 B 上每隔 10 秒新建一个 operator，共六次，观察连接准入。新连接探测写入 `probe-events.csv.gz`，定时导出写入 `export-events.csv.gz`，后者与主 events 一起汇总。工具在已建立的 SSH 通道中测量五次时钟偏差，选往返时间最短的一次；启动计划换算到 B 的时钟，事件汇总按起止时差线性校正。A 的 epoch 定义测量窗口，原始事件时间戳与操作实测时间保存在结果中。

采样器读取进程 user/sys ticks、RSS、cgroup memory.current/peak、线程、FD 和本机 :8443 established TCP 数。重启后重新读取 PID；跨 PID 的 CPU 区间留空。memory.peak 为当前 cgroup 生命周期累计峰值。CPU 100% 表示一个逻辑核。采样按单调时钟的固定截止点推进；每行保存实际时间，CPU 按实际间隔计算，平均 CPU 对窗口边界的 tick 变化积分。

结束或收到 SIGINT/SIGTERM 后停止实验进程与 bench unit，然后收集 journal JSON、资源 CSV、loadgen 事件和运行参数，执行 `bench/analyze/journald.ts` 的 extract、summarize、series。`summary.ts` 生成 metrics.json、active.csv、summary.md，并核对 journal 与提取后 HTTP 事件数。Active 根据成功应用握手及断线事件累计；客户端请求起止区间用于计算 logo 并发数。进程遭 SIGKILL、工作站断电或 SSH 不通时需要从工作站手动停止上述隔离单元。

## 容量基线

N=300 和 418 使用已有的 B0 基线与 418 队伍名单；300 只启动前 300 台设备。N=500、600、800、1200 先各自扩充合成名单，再关闭 logo 轮询完成 Enrollment，保存专属快照和 fleet。新增设备的 ETag 从同组织的 B0 缓存复制，组织按设备序号模 140 分配；私有 fleet 数据保留在 B。prepare 要求对应版本目录已有含418个身份的 `baseline.tar.gz` 与摘要；新版本须先在 A 内迁移该起始快照。N=1200 要求 Server 的连接上限高于1200，首次注册的782台仍受64个短期认证名额及640条review registry约束，工具最多等待180秒收敛。

```sh
deno run --frozen --cached-only --no-prompt --allow-read --allow-run --deny-net bench/run/prepare.ts 500 bench/run/experiments/B2-N500-on.toml
deno run --frozen --cached-only --no-prompt --allow-read --allow-run --deny-net bench/run/prepare.ts 600 bench/run/experiments/B2-N600-on.toml
deno run --frozen --cached-only --no-prompt --allow-read --allow-run --deny-net bench/run/prepare.ts 800 bench/run/experiments/B2-N800-on.toml
deno run --frozen --cached-only --no-prompt --allow-read --allow-run --deny-net bench/run/prepare.ts 1200 bench/run/experiments/CMP-pr63-close-B2-N1200-on.toml
```

## 合成 logo 与首次 operator

loadgen 的 `generate-logos` 从聚合 profile 读取尺寸、字节预算和数量，生成 140 张使用图片、额外目录图片及实测 profile.json。尺寸上限为 2048×2048，seed 固定时图片固定。编码数据至少占文件预算的 85%，剩余空间使用合法 RIFF JUNK chunk 补齐；无法满足编码预算时返回错误。

`encoder` 可选 `image`、`libwebp` 和 `libwebp-rgba`。`image` 使用 workspace 的纯 Rust 编码器；后两者使用 [webp 0.3.1 的安全接口](https://docs.rs/webp/0.3.1/webp/struct.Encoder.html)，其 libwebp 源码随依赖构建，编译时需要 C 编译器。生成过程全部由 Rust 工具完成。`detail_shapes` 控制带种子的填充椭圆数量；`noise_spread` 为 0 时噪声连续放置，为 100 时分布到整张图；`webp_method` 为 libwebp 的 0–6 压缩档位，lossless quality 固定为 80。上述字段可由同名 CLI 参数覆盖。

实验 profile 选择 libwebp、55 个椭圆、连续噪声、method=6。按原始目标顺序，每四项中的第四项设置 `alpha=true`，共 35/140 张；这些图片增加 8 像素单元的透明度纹理，透明度范围为 224–255，纹理相位取自 seed。透明度标记随目标一起进行带种子的 shuffle。`libwebp-rgba` 将该纹理应用于所有目标，可用于诊断长尾。图片对应的字节预算和尺寸仍取自原始聚合目标。

```sh
natsume-loadgen generate-logos --profile bench/run/experiments/logo-profile.json --output-dir logos --directory-count 1033 --seed 418
natsume-loadgen decode-logos --directory logos --synthetic --rounds 9
natsume-loadgen setup --server https://SERVER:8443 --trust-root ca.crt --operator-login synthetic-admin --operator-password-file operator-password --output-dir fixture --logo-directory logos --reuse-logos --teams 418 --schools 140
```

`decode-logos` 使用与 v2.10.3 Server 相同版本的 image 解码器，顺序执行一次预热和指定次数的测量，包含文件读取、64 MiB 解码分配限制及 4096 像素边长、4 Mi 像素总量检查。它输出逐文件中位数的 p50/p90 和逐轮整组耗时的 p50/p90；合成图片还输出匿名逐文件样本。测量现场目录时必须增加 `--aggregate-only`，以仅输出聚合统计；默认按文件大小秩选择 140 张，`--sample-count 10000` 可测量整个目录。失败数量按测量轮次累计。建立实验快照时复制原始基线，仅替换合成 logo 目录，并使用新快照名称。

首次 bootstrap 使用 `bootstrap.ts bench/run/experiments/B0-short.toml`。A 须安装 `expect` 和 `openssl`，bench unit 须停止。工具在 A 的 root-only `/run` 临时目录生成 RSA 私钥，只将公钥发给 B；B 读取本地口令并返回 RSA-OAEP 密文；工作站只转发密文。A 的 Expect helper 在内存解密并通过 PTY 回答 Server bootstrap，关闭终端输出，结束后删除临时私钥。口令文件仅保存在 B，口令不进入 argv、日志或结果文件。安装配置与 Server 二进制须在执行前完成。

## CPU 采样分析

perf 的 Server 二进制须包含可解析符号。若现有二进制已剥离符号，使用相同 tag、编译器和 release 优化配置另建 `CARGO_PROFILE_RELEASE_STRIP=none` 版本；调用栈展开不完整时，用 `RUSTFLAGS="-C force-frame-pointers=yes" CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` 补建并以 `call_graph="fp"` 记录，记录二进制 SHA-256 和构建差异。对该版本测量时不得使用其他二进制的地址映射。

```sh
git clone https://github.com/brendangregg/FlameGraph.git target/bench-flamegraph
deno run --frozen --cached-only --no-prompt --allow-read --allow-write=context/bench/data/results --allow-run --deny-net bench/run/perf.ts context/bench/data/results/DATE-B1a-perf target/bench-flamegraph
```

帧指针采样保留 perf.data、perf.script、perf.folded、flamegraph.svg 和 perf-attribution.json；DWARF 二进制采样包含进程栈内存，保留在 A，仅收集符号化文本，并记录 FlameGraph revision。归因按整个采样栈的符号匹配，输出 decode、sha256、其他 logo、其他代码与未解析样本的互斥份额及 logo 并集，按等权样本计数；decode 包含 natsume_roster 的 decode_logo/decode_raster 调用栈，原始符号中的分号在折叠时转为冒号；优化后的内联与未解析帧影响可识别范围。

## 验证

```sh
deno test --frozen --cached-only --allow-read --allow-write=target --allow-run --deny-net bench/run
cargo test -p natsume-loadgen
cargo clippy -p natsume-loadgen --all-targets -- -D warnings
deno task check scripts
prek run --all-files
```

自动测试仅使用合成配置、统计样本、调用栈和本地协议 fixture，不连接实验机器。真实运行结果写入本地忽略的 `context/`，程序和示例实验配置位于仓库。
