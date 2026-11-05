# 客户端虚拟工位实验

这些脚本由工作站控制两台独立主机：A 运行隔离的 `natsume-bench.service` 并构建比赛镜像，B 运行两台 QEMU 工位。控制流始终经工作站分别 SSH 到 A、B，guest 操作通过 B 的 qemu-guest-agent（QGA）完成。Server 与 Client 使用同一个提交构建的版本，VM 镜像另行记录其提交和内置 Client 版本。

## 前提与目录

工作站需要 Python 3.12、POSIX sh、OpenSSH、ShellCheck，以及构建 Natsume 所需的 Deno/Rust/Web 工具链。A 需要独立的实验 Server、有效的 TLS/CA、合成名单与校徽；首次 bootstrap 需要 expect 和 OpenSSL。B 需要 Ubuntu noble 对应的 QEMU/KVM、qemu-utils、OVMF、numactl、Python 3.12、root 权限和足够的磁盘空间。CPUSET、NUMA 节点和 OVMF 路径须按 B 的实际硬件填写。

所有 SSH 使用 `BatchMode=yes`、`StrictHostKeyChecking=yes`、`UpdateHostKeys=no`、`ForwardAgent=no`。操作前通过自己的管理流程建立并核对主机信任；脚本不接受未知主机密钥，不转发 SSH agent。A、B 与 guest 均启用时间同步；冷启动可能发生 UTC 校准，单 boot 内优先使用 monotonic 时间。

B 的 VM_ROOT 及其祖先须为可信 root 目录，VM_ROOT 为 root:root 0700。ISO、模板盘和封存 VARS 为 root:root 0400；克隆目录为 0700，overlay 和每台 VARS 保持私有可写。RUN_ROOT 是 B 上保存 API 辅助脚本的目录，应由 B 的管理账号管理；AUTH_DIR 只在 B 保存 `control-ca.crt` 和权限收紧的 `operator-password`。这些目录与配置中的路径一致。

A 的实验 Server 使用独立二进制、配置和数据目录，由 `natsume-bench.service` 管理。生产 `natsume-server` 必须 disabled、MainPID 0。部署、bootstrap 和数据准备参照 [Server 运维文档](../../docs/operations-deployment.zh-CN.md)、[实验运行工具](../run/README.md)；本目录不安装或更改生产 Server。

## 统一配置

将 [env.example](env.example) 复制到仓库外，修改为自己的环境，设置 `NATSUME_BENCH_ENV` 指向该文件。示例使用文档地址 `192.0.2.10` / `192.0.2.20` 和合成 UUID，没有作者的实验地址或身份。

```sh
cp bench/vm/env.example /absolute/private/location/bench.env
chmod 600 /absolute/private/location/bench.env
export NATSUME_BENCH_ENV=/absolute/private/location/bench.env
sh bench/vm/c3-run.sh --dry-run
```

格式为一行一个 `NATSUME_BENCH_KEY=value`，可使用单引号或双引号包住空格，不执行 shell 展开、命令替换或变量引用。同名环境变量优先于文件；也可完全通过环境变量提供配置。缺失、重复、未知字段或不合法值会报错，不回退到作者的环境。所有字段均须填写，秘密内容不得放入此文件。部署目录使用不含空格或 shell 特殊字符的绝对路径；SERVER_LOGO 可含空格。

| 配置项，均带 NATSUME_BENCH_ 前缀 | 内容 |
| --- | --- |
| A、B | 不同主机的 `user@hostname` SSH 地址 |
| VM_ROOT | B 上 root-only VM 根目录 |
| RUN_ROOT、AUTH_DIR | B 上公共控制脚本目录、认证材料目录 |
| SERVER_URL | 无凭据、路径、查询和片段的 HTTPS origin；端口由 URL 指定，省略则为 443 |
| ADMIN_LOGIN | 已 bootstrap 的实验管理员登录名；口令从 B 的文件读取 |
| VM1、VM2 | 两个不同的 VM 名，只含字母、数字和连字符 |
| DEVICE1、DEVICE2 | 对应 Server 已注册设备的规范 UUID，显式提供，不按瞬时连接状态猜测 |
| SEAT1、SEAT2 | 对应合成座位，格式 S 加五位数字 |
| SERVER_DATA、SERVER_LOGO | A 上隔离实验数据目录，以及 C5 要替换的合成学校校徽绝对路径 |
| CPUSET、NUMA_NODE | B 上 QEMU 的 CPU 亲和列表与内存 NUMA 节点 |
| OVMF_CODE | B 上只读 OVMF 固件路径 |
| SSH_PORT_BASE、VNC_DISPLAY_BASE | 工位 index 加到这两个基数；VNC 的 TCP 端口再加 5900，仅监听 loopback |
| TEMPLATE_SSH_PORT、TEMPLATE_VNC_DISPLAY | 模板安装时的 loopback SSH 端口与 VNC display，须空闲 |
| MAC_PREFIX | 五个十六进制字节的 MAC 前缀，最后一个字节由工位 index 生成 |

VM1/VM2 是实验角色，不要求物理 VM 名为固定字符串。克隆 index 仍使用 1、2，并决定 MAC 与 loopback 端口。初次安装尚无 Device ID 时可保留示例的合成 UUID 完成模板和启动准备；设备注册后须用 Server 上的真实 ID 替换，再运行采样。脚本会校验 UUID 格式，API 仍会拒绝不存在或不可访问的设备。

所有更改为配置驱动的入口支持 `--dry-run` 或 `--print-config`，放在参数任意位置均先预览配置再退出。预览只打印上述公开字段，不连接 SSH/QGA、不读取认证文件、不提交目标、不创建采样目录。源脚本与展开后的部署脚本均支持预览。

### 部署公共辅助脚本

工作站运行器在传输辅助脚本前用 `bench_env.py` 展开公开配置；部署副本自带这些非秘密值，因此 sudo/systemd 不需要继承工作站环境或安装配置模块。直接从源目录运行时仍读取 env 文件或环境变量。修改配置后重新部署副本，避免同一批实验混用旧值。

例如在工作站生成公共部署副本，再经严格 SSH/scp 分别送到配置中的 B 目录。生成文件放在仓库外；输出目录也会包含自己的主机和设备信息。

```sh
mkdir -m 700 /absolute/private/location/vm-scripts
for script in bench/vm/*.sh; do
  python3 bench/vm/bench_env.py render "$script" > "/absolute/private/location/vm-scripts/$(basename "$script")"
done
chmod 700 /absolute/private/location/vm-scripts/*.sh
```

将 host/QGA 辅助脚本放入 B 的 VM_ROOT，至少包括 `run-vm.sh`、`qga.sh`、安装与克隆脚本；将 `target.sh`、`fault-api.sh`、`network-operations.sh` 放入 RUN_ROOT。API 辅助脚本在 B 执行并读取 AUTH_DIR，工作站只接收白名单结果。工作站运行器及 `bench_env.py` 保留在本仓库目录，不能把需要工作站配置的控制器当作 guest 探针执行。

## 准备顺序

### 1. 在 A 构建镜像

在 A 使用镜像仓库的独立、已提交检出，先读该版本的 Makefile、docs/build-layers.md 和 docs/admin.md。依该版本文档在 A 内准备 secrets、args、Natsume 配置与许可证等输入，权限保持或收紧；不把内容带到工作站。已验证的 contestant seeded ISO 顺序如下，目标名称以所选镜像版本的文档为准：

```sh
make deps
uv sync --frozen
make fetch
make seed EDITION=contestant
make bootstrap
make image STAGE=minimal,standard,extra,live
make iso EDITION=contestant SEED=out/autoinstall-contestant.yaml
```

耗时阶段放入 A 的 detached transient systemd 单元，日志只选固定阶段进度。需要代理时按镜像项目的代理传播说明配置。镜像中的 Client 依镜像项目自己的声明获取；后面再安装与 Server 匹配的 Client deb。

A 上计算 ISO 大小和 SHA-256，经工作站管道直接转到 B，在 B 复核。ISO 不在工作站落盘，旧 ISO 和模板保留。只读核对镜像版本、Client 版本和策略/权限元数据，不打印 autoinstall 或 guest 配置。

### 2. 安装并封存模板

在 B 的新 root-only 工作目录中，用已经展开配置的公共脚本执行：

```sh
sudo systemd-run --unit=natsume-vm-template-install \
  --property=UMask=0077 --property=KillMode=control-group \
  --property=TimeoutStopSec=120 \
  /absolute/vm-root/install-template.sh \
  /absolute/contestant.iso /absolute/template-work 0 0 64 \
  /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_VARS_4M.fd
```

位置参数依次是 ISO、模板工作目录、CPU NUMA 节点、内存 NUMA 节点、磁盘 GiB、OVMF CODE 和原始 VARS。选择 B 上有效节点，保留至少磁盘容量加 5GiB 的空间。脚本从 ISO 只读提取标准 Ubuntu 安装项，不执行其内容；serial 输出在写盘前仅保留固定里程碑和行数。

QEMU 的成功退出只表示进程已结束。安装后离线核对系统与安装完成状态、Natsume identity/keys/control 目录为空、原生 contest-reset-policy 存在、信任链 `/`、`/etc`、`/etc/natsume` 为 root 所有且无 group/other 写，再将 `template.qcow2` 和 `install/OVMF_VARS.fd` 设为 0400。重试使用新目录。

### 3. 克隆与 QGA

准备 noble 对应的 qemu-guest-agent 和缺失依赖 deb，保存在 B。VM 关机时执行：

```sh
sudo /absolute/vm-root/clone-vm.sh /absolute/vm-root station01 1 /absolute/agent-debs /absolute/template-work
sudo /absolute/vm-root/clone-vm.sh /absolute/vm-root station02 2 /absolute/agent-debs /absolute/template-work
```

将名字替换为配置的 VM1/VM2。脚本锁定克隆操作，使用空闲的 `/dev/nbd0` 挂载第 2 分区，清空 machine-id，离线安装 QGA，并再次确认无 Natsume 身份后卸载。它创建各自的 overlay、VARS 和 UUID。按相同 name/index 重建对照工位时，在首次启动前保留原 UUID，便可保持 SMBIOS、board serial、磁盘 serial 和 MAC。

通过 `run-vm.sh VM_ROOT VM_NAME` 在 `natsume-vm-VM_NAME.service` transient 单元中启动。默认实验规格仍为 2 vCPU、4GiB、64GiB 克隆盘，CPU/NUMA 亲和来自配置。QEMU user networking 的 netdev ID 固定为 `net0`，monitor 和 QGA socket 位于各克隆目录；此 ID 是 launch 与断网探针共享的接口约定。

### 4. 安装匹配的 Client

按 [打包说明](../../packaging/README.md) 在工作站准备、构建 Server/Client 并打包 Client。将公开 deb 经工作站送到 B，在 B 用 `dpkg-deb -I` 检查依赖；缺失依赖从对应发行版取得，留在 B 离线安装。

A bench 保持停止，直到两台 Client 均完成安装：

```sh
sudo /absolute/vm-root/install-client.sh /absolute/vm-root station01 /absolute/client.deb /absolute/dependency-debs
sudo /absolute/vm-root/install-client.sh /absolute/vm-root station02 /absolute/client.deb /absolute/dependency-debs
```

脚本通过 QGA 传入 deb、验证 SHA-256、在 guest 内离线安装，并输出版本、时间同步状态。产品配置迁移只按该版本的字段契约操作，不输出配置值。新镜像应原生提供 reset policy、权限和 CUPS 等契约；`apply-image-policy.sh` 仅用于文档明确允许的旧工位维护，不能在新模板上自动补写以掩盖镜像差异。

### 5. 初始化 Server、注册与绑定

先按 Server 文档准备独立实验数据目录和 bench 单元，确认生产 disabled/MainPID 0。使用 [bench/run 的加密 PTY bootstrap 流程](../run/README.md#合成-logo-与首次-operator)，其独立实验配置的 A/B/server_url 与本目录配置保持一致。口令仅保存在 B，工作站只转发密文。

在 B 用 loadgen setup 创建两队、两校合成数据，参数包括 `--teams 2 --schools 2 --reuse-logos`；认证使用 `--operator-password-file`，CA 使用 `--trust-root`。两张公开合成校徽安装到 A 的隔离数据目录，C5 的 SERVER_LOGO 对应其中一校。配置 ADMIN_LOGIN 必须与 bootstrap 的管理员一致。

两台 Client 安装后才启动 A bench。通过 Server 管理 API/Panel 核对注册设备的硬件身份与 VM 对应关系，将返回的实际 Device ID 写入仓库外配置。通过真实 Agent 的 Binding 输入或 QEMU monitor `sendkey` 输入配置座位并提交；C1 的 rebind 已包含该输入路径。绑定不自动把前台切为 Contest。

Home 为 recovery_required 时先按 [运行环境契约](../../docs/client-environment.zh-CN.md) 和 [架构 Home 契约](../../docs/architecture.md) 使用受支持的 reset_home 恢复，不伪造 completion。采样前要求 active、Binding assignment/credential applied、Gateway ready、Home steady、Session running 和两个 ready=true。检查 dry-run 的主机、路径和 Device ID 后再执行运行器。

## 运行器与实验

以下命令从仓库根目录执行，OUTPUT 使用新目录。涉及故障的运行器还要求预先在目录写 `expectations.md`，记录当前源码/架构依据与预期。VM、Server 的启动/停止前提和结束职责见表；运行器不会改写生产单元。

| 实验 | 入口与参数 | 前提与行为 |
| --- | --- | --- |
| C1 收敛时延 | `c1-run.sh 1\|2 COUNT OUTPUT [COOLDOWN_SECONDS]` | 选择统一配置的 VM 槽位；两机顺序执行；每轮 Contest、Waiting、terminate、Home reset、解绑后经 Agent 回绑；默认冷却 3s |
| C3 Home 写入量 | `c3-run.sh OUTPUT [--combined\|--large-small]` | VM1 运行且已绑定；empty/100MB/1GB/100k 小文件；可加组合或 500k/1M 条件，保持已有次数与种子 |
| C5 资源占用 | `c5-run.sh OUTPUT` | VM2 已绑定；Waiting/Contest/编译各 300s，logo 600s；替换公开合成校徽后恢复原内容 |
| C6 开机 / C2 冷热切换 | `c6-run.sh C6_OUTPUT C2_OUTPUT` | VM2 关机、A bench 运行；10 次冷启动，各首次往返再加 4 次热往返；临时 host unit 结束后移除 |
| CR1 Helper SIGKILL | `fault-run.sh CR1 OUTPUT` | 两机运行供探针安装；VM1，500k 文件，Applied 后约 3s/7s 杀 Helper |
| CR2 断电 | `fault-run.sh CR2 OUTPUT` / `CR2b OUTPUT` | VM1，500k 文件；约 3s/7s 或单次 5s；离线 qemu-img check 通过才重启 |
| CR3 断网 | `network-run.sh CR3 OUTPUT` | VM2 Contest；30s、300s，长轮离线提交 Waiting；finally 恢复 net0 |
| CR4 离线启动 | `network-run.sh CR4 OUTPUT` / `CR4-link OUTPUT` | VM2 先运行且就绪；关机后停 Server 或开机断链约 120s，再自动恢复 |
| CR5 Caddy 故障 | `fault-run.sh CR5 OUTPUT` | VM2 Contest；三次 Caddy SIGKILL、一次 admin socket mode=000，并恢复权限或确认重建 |
| CR6 GDM 准备超时 | `gdm-run.sh OUTPUT` | VM2 关机；开机与 terminate 后各暂停一次 gdm greeter compositor，35s 超时后按 PID/start 恢复 |
| CR7 Home 准入 | `admission-run.sh OUTPUT` | VM1 运行；500k 文件，基线/Applied/Verified 的只读锁、permit/window 检查及前后 IsHomeReady |

C1 已统一到配置，不再接收独立 A/B/路径/设备/座位位置参数。例如 `sh bench/vm/c1-run.sh 1 20 OUTPUT 3` 和 `sh bench/vm/c1-run.sh 2 5 OTHER_OUTPUT 3` 分别选择 DEVICE1/SEAT1 和 DEVICE2/SEAT2。原采样次数、收敛判断、冷却和输出字段保留。

CR6 的 60s 连续故障冷却与每 boot 一次预算属于 Waiting 自动重建；Contest 准备失败仍按原 plan retry 重试。CR7 的 GATE_ALLOWED/BLOCKED 是只读守卫结果，真实 PAM worker 身份和窗口内完整认证未由该探针覆盖。按每次实验实际观测给出判定。

### 安装与采集辅助入口

`install-template.sh`、`clone-vm.sh`、`install-client.sh`、`qga.sh`、`apply-image-policy.sh` 保留各自明确的位置参数，路径与名称由调用方提供。它们拒绝不可信目录和不合法名字；安装操作只能对关机或明确处于维护状态的实验工位执行。来宾内固定账号 teams/waiting、产品路径 `/etc/natsume`、systemd 单元和 D-Bus 名称来自镜像/Client 契约，未改成任意参数。

`collect-events.sh server A SINCE [DEVICE_ID]` 与 `collect-events.sh guest B VM_ROOT VM SINCE` 在源端筛选事件。`collect-home.sh B VM_ROOT VM SINCE [UNTIL]` 导出 Home 的固定分项，`collect-retry.sh` 具有相同的 guest 参数，保留固定错误分类。`network-operations.sh DEVICE FROM_MS [TO_MS]` 只导出选定的运营事件、时间和固定 reason；`target.sh DEVICE ACTION [waiting|contest]` 在 B 提交并等待原有收敛条件。

`helper-cpu.sh`、`c5-sample.sh`、`c6-probe.sh`、`network-probe.sh`、`observe-home.sh` 和故障探针由 QGA 在 guest 执行。`network-clock.sh` 在 B 通过 guest-get-time 输出数值校准；`network-link.sh VM off|on` 控制 net0。独立运行时先根据脚本 Usage 提供参数，勿将任意命令或未过滤日志作为 QGA 返回内容。

## 秘密、采集与收尾

ISO 的 autoinstall、guest 的 `/etc/natsume/` 内容、密钥证书、口令哈希、许可证和 SSH key 不进入输出。ISO、模板、克隆、VARS 和截图保留在 B；A 的镜像构建目录可保留源 ISO。管理员口令只在 B 的认证目录，API 登录和 bootstrap 均从 B 读取，不放入 argv、环境配置、仓库或结果文件。

配置预览只列公开配置项。journal 先在 A 或 guest 内按事件名、固定枚举、digest、plan 和数值耗时/计数白名单筛选，再经 SSH/QGA 返回；不得把整行 journald、错误正文、HTTP 正文或配置文件内容传到工作站。采集器没有因去硬编码修改过滤白名单或输出格式。

故障后检查 Home epoch/代次数、测试目录残留、permit/window、Server Actual 和绑定；确认所有 SIGSTOP 的 PID/start 身份已恢复或终止，Caddy 权限/哈希已恢复，链路为 on。通过 QGA guest-shutdown powerdown 正常关机，等 VM 单元 inactive/MainPID 0，再离线 qemu-img check。停止 A bench，生产保持 disabled/MainPID 0，保留绑定、模板和原始数据。

## 本地验证

```sh
sh -n bench/vm/*.sh
shellcheck -s sh bench/vm/*.sh
PYTHONPYCACHEPREFIX=/tmp/natsume-bench-pycache python3 -m py_compile bench/vm/*.py
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s bench/vm -p 'test_*.py'
git diff --check
prek run --all-files
```

`test_bench_env.py` 覆盖缺失配置、环境变量覆盖、拒绝 URL 凭据/非法端口、配置不执行命令、源入口与部署副本 dry-run 不联网及不创建采样目录，以及自定义 VM/路径进入远程命令的情况。上述 dry-run 和本地检查不运行实验。

## Server 故障实验 R1 / R2 / R6

入口为 `server-run.sh` / `server_driver.py`，同样读取统一配置并支持 `--dry-run`。要求隔离 Server 起初只有配置中的两台已绑定设备，A bench 停止、两台 VM 关机；先在各输出目录写好源码依据和预期。每次使用新的输出目录和 B 上新的 `RUN_ROOT/server-reliability` 目录，已有的身份与结果应封存后再运行。

```sh
sh bench/vm/server-run.sh --dry-run
sh bench/vm/server-run.sh prepare /absolute/output/R6 station03 \
  /absolute/template-work /absolute/client.deb /absolute/agent-debs
sh bench/vm/server-run.sh R1 /absolute/output/R1
sh bench/vm/server-run.sh R2 /absolute/output/R2
sh bench/vm/server-run.sh R6 /absolute/output/R6 station03
sh bench/vm/server-run.sh finish /absolute/output/R6 station03
```

`prepare` 用模板创建 index 3 的独立克隆，复制 VM1 的 UUID，并将 `run-vm.sh WORK NAME [IDENTITY_NAME]` 的第三参数设为 VM1，使 SMBIOS UUID、board serial、disk serial 相同；MAC 和端口仍由 index 3 决定。此参数只用于身份故障注入，普通启动省略即可。克隆在 A bench 停止期间离线升级到传入的 Client deb，随后关机。原两台启动后分别进入 Contest 和 Waiting。

准备阶段还启动 16 台 loadgen 设备，使用 S10000 起的未映射座位，不导入名单、不与两台真实座位竞争；它们用于控制连接恢复，不作为已绑定工位的会话就绪样本。自动审批窗口只在这批合成设备初次注册期间打开，全部 active 后关闭。loadgen 使用 AUTH_DIR 下的既有 `natsume-loadgen` 和公开 CA；控制身份、完整压缩指标和日志留在 B。

R1 做两次 SIGKILL，停机后只读执行两个 SQLite 库的 `integrity_check`，5 秒后启动；采集全部设备 attach 与两台 VM 的资源、前台和 epoch。Server 恢复判据是四类资源 converged、Contest ready；同时记录 Waiting ready。Contest 前台下 Waiting 新首帧尚未确认时，该值可能为 false。R2 只隔离 VM2 的 QEMU net0，分别 30/90 秒，期间提交 Contest 目标，finally 恢复链路。R6 在 closed 窗口下分别测试 VM1 在线、关机两种情况，只拒绝克隆的同硬件待审请求，不批准控制密钥替换。

R1 完成后可在 B 停止 `natsume-bench-server-loadgen.service`，再执行已展开的 `server-fleet.sh`，把白名单指标保存到 R1 输出目录。`server-api.sh` 只导出规范设备/硬件 UUID、固定状态与数值；不导出 candidate key、网络地址、名单、credential 或错误正文。`finish` 将两台恢复 Waiting、保留绑定，关闭注册窗口、拒绝请求后核实无待审，禁用本次新增的 16 台合成设备，正常关闭三台 VM、停止 loadgen/Server，最后删除专用克隆目录。中断后先恢复 QEMU 链路，再运行 `finish`；不得删除仍在运行的克隆盘。

## SQLite、Journal 与 authority 故障 R3 / R7 / R4 / R5

入口为 `resilience-run.sh` / `resilience_driver.py`，继续读取同一配置，支持 `--dry-run`。配置的 SERVER_DATA 必须是隔离变体下的 data 目录，业务库和 journal 都须位于其中；生产 Server 保持禁用且无进程。注入前在各输出目录保存源码依据和预期，先关闭实验 Server 与 VM。使用新输出目录，并保证 B 上本批 loadgen 的输出目录未存在。

```sh
sh bench/vm/resilience-run.sh --dry-run
sh bench/vm/resilience-run.sh prepare /absolute/output/R3
sh bench/vm/resilience-run.sh R3 /absolute/output/R3
sh bench/vm/resilience-run.sh R7 /absolute/output/R7
sh bench/vm/resilience-run.sh R4 /absolute/output/R4
sh bench/vm/resilience-run.sh R5 /absolute/output/R5
sh bench/vm/resilience-run.sh finish /absolute/output/final
```

prepare 在 A 停服期间复制 data 到同变体下的 root-only 备份目录，把数据库位置及原权限记录为 A 本地 0600 元数据，再启动 Server/VM。现有备份不覆盖，下一批实验应先在 A 封存备份与本地元数据。R3 暂停 Daemon 调度、提交前台目标，取得业务库写锁后 SIGCONT；锁由 natsume-server 账号持有，7/36 秒后回滚，transient 单元另有自动超时保护。finally 恢复暂停进程。当前日志格式没有输出 db.transaction 的 span close，无法取得的 permit/pool/write-lock 数值需在报告中列出。

R7 注册四台未绑定合成设备，seed=2026110417、座位 S11000 起；停止普通模拟器后，用 `control-probe.sh` 从 B 本地身份文件合法 Resume。探针实现最小 WebSocket/Protobuf 封装，按协议的 SHA-256 transcript 摘要签名；只输出固定阶段、错误类和计数，不导出密钥或完整 snapshot。两个设备在每 50ms 上报 ClientState 时分别 disable/revoke，终态保留。

R4 停服后只对 journal 文件和存在的 WAL/SHM 做只读 bind mount，不改业务库、Server 配置或原权限；Server 启动后观察 degraded，目标仍继续收敛。finally 卸载单独文件，等待 60 秒周期自动恢复，导出丢失计数与 journal_dropped 摘要。此方法覆盖打开失败，不覆盖磁盘预算暂停的 paused 路径。

R5 另启动两台普通 loadgen（seed=2026110418，S12000 起），并保留一台正常读取的探针。慢读探针收到首个目标后停止读取，每秒上报一次，首次 Ping 在 30 秒；B 的 Operator 对它提交 1000 个交替前台目标，促使下行积压。两台 VM 分别采集注入前 W→C、burst 内 C→W、注入后 W→C，并记录 A 同机 dispatch→匹配 Actual 的时延。结果按实际断开 reason 判定，不能把探针自身写超时当成 Server 慢读隔离成功。

finish 停止本批 loadgen、关闭自动审批、将 VM 恢复 Waiting、确认 Home 与 completion，再关机；仅把本批已知 seed 的 enabled 合成设备置 disabled，保留 revoked。卸载 journal 文件、释放 SQL 锁、停止 Server 后做两个 SQLite 库的只读 integrity_check，并核对 journal 原 owner/mode。备份及密钥始终留在 A/B。R6 开窗变体须先确认 Client 支持的换钥恢复流程，不能把重新批准退休 key 当作恢复方案。
