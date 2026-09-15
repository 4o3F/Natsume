# 原生 Wayland KMS 桌面串流方案

本文为**获得赛事主办方书面授权的受管比赛工位**提供一个可选的桌面观测方案。目标是把当前物理输出的画面低延迟发送到指定的运维接收端，同时不改变 GNOME Wayland 会话、不注入桌面门户授权流程，也不录制音频或输入。

它不是 Natsume 的默认功能、不是 Agent/Helper 的职责，也不随 Client Deb 或镜像自动启用。是否启用、接收端地址、保留策略和可观看人员必须由部署方单独批准并记录。

## 1. 结论与边界

### 1.1 采用的路径

```text
当前 seat0 的 KMS scanout
  -> FFmpeg kmsgrab（root，CAP_SYS_ADMIN）
  -> 软件 H.264（libx264）
  -> MPEG-TS over SRT
  -> 固定的赛事接收端
```

`kmsgrab` 读取的是 KMS CRTC/plane 的 scanout framebuffer，而不是连接到某个用户的 Wayland socket；因此它适合抓取当前实际显示的 GNOME 输出。FFmpeg 官方文档说明该输入需要 DRM master 或 `CAP_SYS_ADMIN`，且默认设备为 `/dev/dri/card0`；抓帧频率是定时采样，超过实际刷新率不会产生更多新画面。[FFmpeg kmsgrab 文档](https://ffmpeg.org/ffmpeg-devices.html#kmsgrab)

这条路径不会触发 PipeWire/xdg-desktop-portal 的屏幕共享请求，因而在本次 Ubuntu 24.04.5 GNOME Wayland QEMU 验证中未出现 GNOME 的“正在共享屏幕”提示。本次已实际验证：`kmsgrab` 可抓到 1280×800 KMS 画面，`hwdownload,format=bgr0` 加 `libx264` 可输出 MP4。**尚未在该 ISO 上完成端到端 SRT 接收验收**；第 7 节将其列为上线门槛。

### 1.2 明确不做的事

- 不用 `x11grab`、Xwayland 或 `wf-recorder`；前者不适用于原生 Wayland，后者依赖 wlroots 协议而 GNOME/Mutter 不提供。
- 不调用 portal、GNOME Remote Desktop、VNC 或 RDP；这些方案可能要求用户授权或显示共享状态。
- 不抓音频、摄像头、剪贴板、按键、鼠标事件或任意用户文件。
- 不改变 GDM、GNOME、Mutter、Natsume Agent/Helper 或 Home reset 流程；服务只读取 KMS 设备并向固定接收端单向发送媒体。
- 不允许由比赛用户传入输出 URL、编码器选项、DRM 设备路径或 systemd unit 名。

### 1.3 已知限制

- KMS 抓到的是当前 scanout，不是单独的 Wayland 窗口；切换前台会话或显示器时，接收端看到的是切换后的实际画面。
- 一张卡的多显示器、多 CRTC 或非线性/modifier framebuffer 必须在目标硬件实测。FFmpeg 的 `hwdownload` 仅适用于可映射的线性 framebuffer；否则可能花屏或失败。[FFmpeg 示例](https://ffmpeg.org/ffmpeg-devices.html#kmsgrab)
- 本文的基线是软件 `libx264`。硬件 VAAPI/NVENC 编码可在基线验收通过后单独评估，不应阻塞首个交付。
- KMS 权限、GPU 驱动和 FFmpeg 编译选项随镜像和硬件而变；不能以本次 QEMU 结果替代目标工位验收。

## 2. 设计参数

| 项目 | 基线 | 原因 |
| --- | --- | --- |
| 视频源 | `/dev/dri/card0` 的首个活动 plane | 与当前物理输出一致，避免用户会话 socket 依赖 |
| 帧率 | 30 fps | 比 60 fps 降低 CPU/带宽，仍足以观察竞赛桌面 |
| 编码 | `libx264`、`veryfast`、`zerolatency` | 首个版本不依赖特定显卡编码器 |
| 码率 | 3 Mbit/s CBR 上限 | 1080p 文本桌面的起点；按实测调整 |
| GOP | 60 帧（约 2 秒） | 便于接收端快速恢复和控制延迟 |
| 容器／传输 | MPEG-TS over SRT caller | 单向出站连接，接收端固定监听；SRT 支持延迟和加密参数 |
| 音频 | 关闭 | 仅满足画面观测需求 |
| 服务身份 | root system service | `kmsgrab` 需要 DRM master 或 `CAP_SYS_ADMIN` |

SRT URL 使用 `srt://host:port?options` 格式，支持 `caller`、`listener`、`rendezvous` 三种模式。这里固定工位为 `caller`，接收端为 `listener`。`latency` 是微秒级重传缓冲，`passphrase` 仅在 `pbkeylen` 非零时启用，长度必须为 10 至 79 个字符。[FFmpeg SRT 协议文档](https://ffmpeg.org/ffmpeg-protocols.html#srt)

## 3. 部署前提

1. 仅在已获授权的比赛工位启用；部署方确定接收端、访问人员、赛事结束后的停用时间和视频保留期限。
2. 工位运行官方 GDM/GNOME 原生 Wayland，且目标输出实际经 `/dev/dri/card0` scanout。
3. 安装包含 `kmsgrab`、`libx264` 和 `libsrt` 的 FFmpeg 包；不得假设 ISO 已预装。
4. 接收端必须先提供固定 SRT listener、受控存储和受限观看入口；工位不对公网监听端口。
5. 接收端地址只接受部署输入，不放入 Natsume 的 `config.toml`、Home 或用户可写目录。

在目标机维护窗口执行以下只读预检：

```sh
test -c /dev/dri/card0
ffmpeg -hide_banner -devices | grep -F 'kmsgrab'
ffmpeg -hide_banner -protocols | grep -F srt
loginctl list-sessions
```

前三项分别证明 DRM 设备、输入设备和 SRT 协议存在；最后一项用于在验收记录中关联当前受管图形会话。任一项失败即不启用服务，不能回退到 portal、VNC、RDP 或 X11 抓屏。

## 4. 固定接收端契约

接收端由赛事运维单独建设，最小契约如下：

| 方向 | 契约 |
| --- | --- |
| 工位 → 接收端 | 仅允许向批准的主机和 UDP 端口发起 SRT caller 连接 |
| 编码 | H.264 视频、无音频、MPEG-TS 封装 |
| 加密 | `pbkeylen=32` 且双方使用同一独立 passphrase；passphrase 不写入 unit 或日志 |
| 身份区分 | 每台设备使用不同 SRT stream id 或不同接收端端口；其映射由接收端保存 |
| 可用性 | listener 必须在工位服务启动前就绪；断链后由工位 systemd 有界重启 |
| 存储 | 默认只做实时观看；若接收端保存视频，保留期和删除流程由赛事运维另行定义 |

接收端 listener 的示例 URL（仅说明格式，不可直接复用占位值）：

```text
srt://0.0.0.0:9000?mode=listener&latency=200000&transtype=live&pbkeylen=32&passphrase=REPLACE_WITH_APPROVED_SECRET
```

工位不得在 shell 历史、仓库、镜像层、用户 Home 或 journal 中保存 `passphrase`。FFmpeg 的 SRT URL 最终会作为其进程参数出现，因此**不能把这点伪装为密钥永不在进程参数中出现**；第 5 节只把配置文件限定为 root 可读，且上线验收必须证明比赛账户不能读取该 root 进程的 `/proc/<pid>/cmdline`。具备 root 权限的维护人员仍可读取它，这是该 FFmpeg/SRT 直连实现的固有限制；若目标机的既有 `/proc` 策略无法满足该条件，则不得启用本方案。

## 5. 工位安装与 systemd 配置

以下配置是部署模板。`collector.example.invalid`、端口、设备标识和密钥必须由部署输入替换；不要把示例文件直接制作进公共 ISO。

安装运行依赖：

```sh
sudo apt-get update
sudo apt-get install --yes ffmpeg
```

创建 `/etc/natsume/kms-stream.env`，权限必须为 root:root、`0600`：

```ini
DRM_DEVICE=/dev/dri/card0
SRT_URL=srt://collector.example.invalid:9000?mode=caller&latency=200000&transtype=live&pbkeylen=32&passphrase=REPLACE_WITH_APPROVED_SECRET
```

创建 `/etc/systemd/system/natsume-kms-stream.service`：

```ini
[Unit]
Description=Authorized KMS desktop stream for the contest workstation
Wants=network-online.target
After=network-online.target display-manager.service
ConditionPathExists=/dev/dri/card0

[Service]
Type=simple
User=root
EnvironmentFile=/etc/natsume/kms-stream.env
ExecStart=/usr/bin/ffmpeg -nostdin -hide_banner -loglevel warning -f kmsgrab -device ${DRM_DEVICE} -framerate 30 -i - -vf hwdownload,format=bgr0 -an -c:v libx264 -preset veryfast -tune zerolatency -pix_fmt yuv420p -g 60 -keyint_min 60 -sc_threshold 0 -b:v 3M -maxrate 3M -bufsize 6M -f mpegts ${SRT_URL}
Restart=on-failure
RestartSec=5
TimeoutStopSec=15
PrivateTmp=yes
ProtectHome=yes
ProtectSystem=strict
ReadWritePaths=/run
CapabilityBoundingSet=CAP_SYS_ADMIN
NoNewPrivileges=no

[Install]
WantedBy=multi-user.target
```

说明：

- `NoNewPrivileges=no` 是有意设置：`kmsgrab` 必须具有 DRM master 或 `CAP_SYS_ADMIN`。不要将该 unit 改为普通用户服务，也不要把 DRM 权限授予 `teams`、`waiting` 或 Agent。
- `ProtectSystem=strict` 使服务不写入系统文件；媒体只经网络输出，日志进入 journal。
- `Restart=on-failure` 只处理异常退出。运维明确停止服务时不会被自动拉起。
- 多显示器或活动 plane 不确定时，先用受控维护验收确定 CRTC，再将 `-crtc_id <实际 ID>` 放在 `-f kmsgrab` 之前。FFmpeg 定义该选项时会使用该 CRTC 的首个活动 plane。[FFmpeg kmsgrab 选项](https://ffmpeg.org/ffmpeg-devices.html#kmsgrab)

加载与启用仅在批准的维护窗口执行：

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now natsume-kms-stream.service
sudo systemctl status natsume-kms-stream.service
```

不要在 unit 中加入 `ExecStartPre` 去修改 GDM、切换 VT、停止 Natsume 服务或重启图形会话。该服务失败时应只影响串流，不应影响比赛桌面。

## 6. 接收端观看与运行观测

接收端应以其批准的媒体服务消费 SRT listener。仅用于验收时，可使用一个具备相同 SRT 参数的播放器连接 listener；例如：

```sh
ffplay -fflags nobuffer -flags low_delay -framedrop \
  'srt://0.0.0.0:9000?mode=listener&latency=200000&transtype=live&pbkeylen=32&passphrase=REPLACE_WITH_APPROVED_SECRET'
```

工位侧只观察服务健康和错误，不记录画面内容：

```sh
sudo systemctl is-active natsume-kms-stream.service
sudo journalctl -u natsume-kms-stream.service --since '10 minutes ago'
```

允许的正常状态是服务运行且接收端有连续画面。`Permission denied`、`No such device`、`Failed to download frame`、持续重连或接收端黑屏都必须作为失败处理，不得通过放宽用户目录权限、改用 portal 或改造 GNOME 来绕过。

## 7. 上线验收

以下项目均须在**实际目标硬件、实际 Wayland GNOME 会话和实际接收端**完成；QEMU 和本地 MP4 仅是前置可行性证据。

| 编号 | 操作 | 通过条件 |
| --- | --- | --- |
| ST-01 | 服务启动后查看接收端 | 画面为当前物理输出，H.264/MPEG-TS 解码稳定，无音频 |
| ST-02 | 在桌面打开、最小化窗口并切换活动概览 | 接收画面同步变化；屏幕上不出现 portal 授权窗或 GNOME 共享状态提示 |
| ST-03 | 在 1080p 下连续观察 10 分钟 | 无花屏、绿屏、持续重复旧帧或服务退出；记录 CPU、码率和端到端延迟 |
| ST-04 | 断开接收端网络 60 秒后恢复 | 比赛桌面保持可操作；服务按 5 秒节奏重连，恢复后接收端重新获得画面 |
| ST-05 | 切换 waiting 与 teams 的正常前台 | 接收端仅呈现当前 scanout；不影响 GDM、Agent、Helper 或 Home reset |
| ST-06 | 重启工位并完成两个原生 Wayland 会话准备 | 图形会话健康后服务可连接；启动早于活动 plane 时允许短暂失败后重试 |
| ST-07 | `systemctl stop natsume-kms-stream.service` | 接收画面停止，比赛桌面及 Natsume 全部保持正常 |
| ST-08 | 审计部署文件与日志 | URL 密钥未出现在仓库、镜像、用户 Home、shell history 或 journal；比赛用户不能读取 root 服务的进程参数 |

建议记录：镜像版本、内核、GPU/DRM 驱动、FFmpeg 版本和构建配置、输出分辨率、平均 CPU、平均/峰值码率、端到端延迟、接收端版本，以及每项验收的时间与结果。不要把本机临时目录、PID 或测试视频作为正式交付证据。

## 8. 性能调优顺序

先完成第 7 节基线后，按下列顺序逐项调整并重新验收：

1. 若 CPU 过高，先把 `-framerate 30` 降到 20 或将码率降到 2 Mbit/s；一次只改一个参数。
2. 若文本模糊或运动画面不足，再提高码率至 4–6 Mbit/s；不要用增大缓冲掩盖丢包。
3. 若网络抖动导致断续，将 SRT `latency` 从 `200000` 提高到 `300000` 或 `400000` 微秒，并记录额外延迟。
4. 若基线稳定但 CPU 仍不能接受，再在独立变更中评估 `hwmap=derive_device=vaapi` 和 H.264 VAAPI；必须保留软件路径为可回退基线。

不建议把帧率直接提高到 60 fps。FFmpeg 明确说明 KMS 采样没有与 page flip 同步，超过实际 framebuffer 更新频率只会得到更多相同的帧。[FFmpeg kmsgrab 帧率说明](https://ffmpeg.org/ffmpeg-devices.html#kmsgrab)

## 9. 停用与回退

赛事结束、授权撤销、验收失败或任何疑似影响桌面稳定性的情况，执行：

```sh
sudo systemctl disable --now natsume-kms-stream.service
sudo rm -f /etc/natsume/kms-stream.env
sudo rm -f /etc/systemd/system/natsume-kms-stream.service
sudo systemctl daemon-reload
```

此回退只移除可选串流服务和其私有部署输入；不修改 ISO、Natsume Client、GDM、GNOME、用户 Home 或接收端历史数据。接收端保存内容的清理按其单独的保留策略执行。

## 10. 实施决策

本方案的首期交付仅包含：FFmpeg 软件编码、固定 SRT 接收端、root-only 配置、单个 systemd 服务和第 7 节验收。多路转码、浏览器观看页、录像检索、硬件编码、动态控制 API、用户级开关及对 Natsume 协议的接入均不在本期范围内。
