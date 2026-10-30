# Natsume 从零部署与运维手册

本文以 natsume-server-setup 完成 Ubuntu 24.04 Server 首次安装，并说明证书来源、离线 CSR、从源码构建 Client Deb、包外输入交接、样机注册验收及备份恢复；手工部署命令保留在附录 A。Server 使用 Release Deb，Client 仅构建 Deb，不涉及 ISO 制作。

## 0. 适用范围与执行顺序

本文描述当前源码对应的部署流程，系统环境为 Ubuntu 24.04 LTS amd64。命令中的 `2.10.3` 为示例版本。正式部署须选定一个已发布版本，Server 包和 Client 源码必须来自同一标签；该标签还须包含 natsume-server-setup 和本文使用的 Deno 构建入口，否则应使用该版本自带的指南。未发布源码的联调须从同一 Git SHA 构建两端，不能将旧 Server Release 与当前分支 Client 拼接部署。

Client 运行验收需要完成配套 GNOME/GDM、原生 Wayland 和 Home 集成的样机；镜像项目负责桌面适配和目标机器验收，要求见第 7 节。

除明确标注的离线 CA 签发外，Server 安装、管理 API 调用和 Client Deb 构建都在同一台 Ubuntu 24.04 Server 上执行。使用同一个具有 sudo 权限的管理员账号，以下 $HOME 均指该账号的 Home；需要服务用户执行的命令会显式使用 sudo -u natsume-server。各节环境变量在同一终端中沿用，换终端后按对应步骤重新设置。

第 7 节标注的 Client 安装/运行检查在比赛机执行，Web/Firefox 操作在实际使用的浏览器中完成；请按步骤和执行位置逐项操作。

| 执行位置 | 职责 |
| --- | --- |
| Server 上的管理员账号 | 显式运行 setup、保存私钥与交接材料、调用管理 API、构建 Client Deb |
| 离线 CA 管理员（如采用 CSR） | 核对请求并直接签发 Server leaf，仅返回公共证书 |
| Server 上的 natsume-server 服务用户 | 初始化数据库、运行服务、读取在线签发密钥和业务数据库 |
| Client 比赛机 | 安装后生成本机身份、注册、绑定工位、运行比赛会话 |

新生成的 CA 私钥以加密 PKCS#8 PEM 在终端展示一次，默认不落盘，管理员须另行安全保存；也可显式选择保存到受限 PKI 目录（默认 /root/natsume-pki）。Server TLS key 和 Local Origin CA 在线副本部署到服务私有目录；Control CA 私钥不交给 natsume-server 服务用户。CA 私钥不进入源码、Deb 或 Client 的部署输入。

执行顺序：

1. 确定地址、域名和期限。
2. 按第 2 节选择生成、导入或离线 CSR 证书来源。
3. 下载并校验 Server Deb，安装后在交互 TTY 执行 sudo natsume-server-setup；CSR 模式签名后续接，直至 bootstrap、systemd 和健康检查通过。
4. 导入比赛数据，从源码构建 Client Deb 并准备包外输入。
5. 如需联调，在配套样机安装后按窗口状态完成自动或人工注册审批，再绑定并验收。
6. 交付包与配置；联调/注册结束后关闭窗口，归档并备份。

Server 的完整 config.toml 包含 DOMjudge 上游地址。bootstrap 创建/迁移全部业务表，并在同一事务中初始化首个管理员和 Runtime Config，无需手工写数据库。

阅读入口：[部署参数](#1-准备部署参数和外部服务) · [证书来源](#2-证书来源决策) · [运行 setup](#4-运行-natsume-server-setup) · [手工部署](#附录-a手工部署流程) · [构建 Client Deb](#6-从源码构建-client-deb) · [部署交接与联调](#7-client-部署输入与首次联调) · [备份维护](#8-日常维护备份和恢复)。

## 1. 准备部署参数和外部服务

### 1.1 填写部署清单

部署前将下表演示值（包括示例地址 192.0.2.10）替换为现场参数；前六项对应 setup 的参数提示，证书路径与口令按来源随后询问。

| 参数 | 演示值 | 约束与用途 |
| --- | --- | --- |
| Server IP addresses | 192.0.2.10,2001:db8::10 | 逗号分隔，去重后 1–16 个；首个用于 Client 与健康检查，leaf 必须覆盖全部 IP SAN |
| HTTPS port | 8443 | 默认 8443，范围 1024–65535；Server、Client、防火墙一致 |
| Gateway hostname | domjudge | 每台比赛机解析到自己的 127.0.0.1 和 ::1 |
| DOMjudge 上游 origin | https://judge.contest.example | 真实 DOMjudge，不能指向 Client loopback |
| contest end | 2026-12-06 18:00（Asia/Shanghai） | 日历面板选择日期，再输入 HH:MM；按 Server 本机时区解释；日期默认本地今天且不早于今天，时间无默认值 |
| Gateway expiry | 2026-12-08 18:00（Asia/Shanghai） | 日期默认比赛结束次日、下限为比赛结束日期；时间默认沿用比赛结束时间；换算 UTC 后至少覆盖结束后 **86400 秒**且仍在未来 |
| 版本 | 选定的已发布版本，命令示例为 2.10.3 | 两端使用同一标签，Client 与镜像集成要求配套 |

setup 对纯 IPv4 使用 0.0.0.0 监听，含任意 IPv6 时使用 [::]；混合地址族须验证目标系统双栈可达性。先用 `timedatectl status` 核对 Server VM 的时区。向导用日期面板和严格两位数的 24 小时制 `HH:MM`（00:00–23:59），按 **Server 本机时区**解释并换算 UTC。例如 Asia/Shanghai 的 2026-12-06 18:00 会保存为 2026-12-06T10:00:00Z。配置和 pending request.toml 仍使用严格 RFC 3339 UTC Z 字符串。

本文以 domjudge 为本机 Gateway 名称，可改用 gateway.contest.example 等名字；保持两端 config.toml 的 Gateway hostname 一致，Client 启动时会自动同步本机 hosts、Firefox 主页和比赛书签。

Client 使用程序内固定的命名空间从硬件证据派生设备 ID。两端必须匹配 Gateway hostname 和两份 CA；比赛结束时间和 Gateway 期限只属于 Server。

### 1.2 网络、时间与 DOMjudge

准备 Server 固定地址或 DHCP reservation、比赛机到 Server 的路由、DNS 和时间同步。各主机检查：

~~~bash
date -u
timedatectl status
~~~

Ubuntu 默认时间同步方案可启用下面命令；若已有 chrony 等方案，保留既有方案并确认实际同步。

~~~bash
sudo timedatectl set-ntp true
~~~

| 来源 → 目标 | 必要访问 |
| --- | --- |
| Client / 管理员浏览器 → Server | TCP 8443，TLS 1.3、HTTP/1.1 和 WSS |
| Client → DOMjudge 上游 | HTTPS 对应端口，通常 TCP 443 |
| Client 浏览器 → 本机 Gateway | loopback TCP 443 |
| 运维网 → Server / Client | 现场 SSH 端口 |
| 各主机 → 基础服务 | 现场 DNS 和时间同步服务 |

用现有防火墙和云安全组放行必要流量，先保留 SSH 入口再调整规则。Client Gateway 不应对其他机器开放。

需要通过SSH批量排障时，在Web的Devices页展开筛选面板中的 **Emergency**，点击 **Export all device IPs** 下载UTF-8文本，每行一个IP。导出包含全部Enabled/Disabled设备的 最近Server观测地址（含离线设备），排除Revoked；搜索和列表筛选不缩小导出范围。文件已去重并 排序，不包含用户名或SSH端口；按现场运维账号和端口交给并行SSH工具使用。

Client同时上报控制连接实际使用的本地IP。双方地址不一致时，设备行的琥珀色警告图标可展开 详情，查看和复制两个地址及记录时间；导出仍使用Server观测IP。地址在断线和Server重启后保留， 新连接会更新整组记录。离线地址是最近记录，记录时间不是最后心跳时间；缺失地址会计数并跳过。 Server升级自动应用第五条migration，保留既有设备和绑定；已有设备在重连后取得地址记录， 旧Client未上报本地IP时显示Not reported。

继续前需自行部署可用的 HTTPS DOMjudge 和比赛账号，并验证自动登录：Gateway 只对 /login 注入 X-DOMjudge-Login 和 Base64 编码的 X-DOMjudge-Pass，DOMjudge 必须支持此约定。

上游必须是 canonical HTTPS origin，例如 https://judge.contest.example 或 https://judge.contest.example:8444，不带结尾斜杠、路径、用户名密码、query 或 fragment。不能填写 /domjudge 或 /api 地址，需要在根路径提供配套页面。

Caddy 用**系统信任**校验上游 TLS。若 DOMjudge 使用额外私有 CA，按 7.3 节给 Client 安装该公共 CA。

## 2. 证书来源决策

### 2.1 明确材料归属

| 材料 | 公共证书接收方 | 私钥位置与用途 |
| --- | --- | --- |
| Control CA | Server、Client、管理员浏览器 | 离线签名方或管理员安全保存；不进入服务目录，直接签发 Server leaf |
| Local Origin CA | Server、Client 系统和 Firefox | 加密备份由管理员保存；P-256 在线副本由 Server 动态签发 Gateway leaf |
| Server TLS leaf/key | leaf 在 TLS 握手提供 | 私钥仅部署到 Server 私有目录 |
| server-root.key | 不分发 | bootstrap 生成，保护数据库中的业务秘密；失败重跑也不得删除 |

Server 只发送一张 leaf，必须由选定 Control CA **直接签发**。Control CA 可用非自签组织 CA 作为直接信任锚；浏览器补链由部署方负责。Local Origin CA 必须是 P-256、basicConstraints CA 且明确包含 keyCertSign，期限覆盖 gateway_not_after。

### 2.2 全新站点：生成

没有既有信任链时，默认生成 Control CA、签发 Server leaf、生成 Local Origin CA。两套 CA 为 P-256、3650 天，leaf 为 min(签发时刻 + 365 天, Control CA 到期)，不足以覆盖 Gateway expiry 时拒绝。确认计划后才展示新 CA 加密私钥并选择保存方式，见第 4.2 节。

### 2.3 既有材料：导入

| Control CA 来源 | Server TLS 可选来源 | 默认 |
| --- | --- | --- |
| 新生成 | 本机签发 | 签发 |
| 导入证书 + 私钥 | 签发 / 导入 leaf + key / CSR | 签发 |
| 仅导入证书，私钥离线 | 导入 leaf + key / CSR | CSR |

Local Origin CA 单独选择生成或导入证书 + P-256 私钥，不支持仅证书。导入私钥原件只在内存使用，不另做备份副本；TLS key 与 Origin 在线副本仍按要求部署。证书须为单张 PEM/DER，不能导入 bundle。支持明文 PKCS#8、PBES2 加密 PKCS#8，以及含命名曲线和公钥的明文 SEC1；拒绝旧式 Proc-Type PEM 与 RSA PKCS#1。格式、KDF 上限和转换命令见 [setup README](../crates/natsume-server-setup/README.md#imported-key-formats-and-one-time-export)。

### 2.4 私钥离线：仅证书与 CSR

Control CA 私钥不能进入 Server 时，选“Import CA certificate only (offline key)”，再选默认的 CSR。只提供证书路径，不提供 CA 私钥。首次运行确认请求后仅生成 /root/natsume-pki/pending/，返回等待签名（退出码 0），不写系统部署输入，也不运行 bootstrap。

~~~bash
sudo natsume-server-setup
sudo ls -l /root/natsume-pki/pending/
~~~

pending 包含 server-tls.csr、control-ca.crt、request.toml 和未加密的 server-tls-key.pk8，目录 root:root 0700，四文件均 0600。只把前三份公共材料经受控渠道交给离线 CA，**私钥留在 Server**。request.toml 记录所有 IP、端口、期限及 CA/SPKI/请求公钥指纹。离线签发命令见[附录 A.10](#a10-离线-ca-签发-setup-生成的-csr)。返回签好的 leaf 后继续第 4.3 节。

### 2.5 安装前校验

setup 在写系统路径前校验完整配置、证书/私钥配对、CA、Origin P-256 和 keyCertSign、当前有效期及对 gateway_not_after 的覆盖、leaf 直接链与全部 IP SAN、serverAuth；leaf 到期不得晚于 Control CA。serve 仅做加载、配对、CA 基本约束和 Origin trust DER 一致性等运行时检查，严格策略差异见 [setup README](../crates/natsume-server-setup/README.md#certificate-sources-and-validation)。后续手工变更不能仅凭 serve 启动成功认定证书正确。

### 2.6 手工部署入口

需要逐项执行 OpenSSL 与部署命令时，使用[附录 A](#附录-a手工部署流程)。不要先按附录落地系统输入，再运行 setup；任一部署文件存在都会被视为已部署。setup 不处理 DOMjudge 上游证书，继续使用第 2.7 节。

### 2.7 用现有 Local Origin CA 签发 DOMjudge 上游证书（可选）

本节保持手工，setup 不签发 DOMjudge 上游证书。DOMjudge 可以使用附录 A.3 或 setup 生成的 Local Origin CA（Gateway CA）。它使用独立叶子私钥；管理员须持有另行保存的加密 CA 私钥。以下沿用附录的 public/ 与 private/ 布局；进入 root 管理终端后，使用安装时的 PKI 目录（默认 /root/natsume-pki 或 --pki-dir）。若用 setup，先在该目录创建 public/ 与 private/（0700），从 /etc/natsume/trust/local-origin-ca.crt 复制公共证书，并把保存的 origin-ca-key.pem 放到受限 private/local-origin-ca-key.pem，不从在线 DER 副本制作交接包。以下签发操作继续在 Natsume Server 上执行，先将 IP 换成实际 DOMjudge 上游地址：

~~~bash
sudo -i
umask 077
NATSUME_PKI_DIR='/root/natsume-pki' # 使用安装时的实际 PKI 目录（或 --pki-dir）
NATSUME_DOMJUDGE_IP='192.0.2.20'
cd "$NATSUME_PKI_DIR"
mkdir -m 0700 domjudge

openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -out domjudge/domjudge-tls-key.pem
openssl req -new -sha256 -key domjudge/domjudge-tls-key.pem \
  -subj '/O=Contest Operations/CN=DOMjudge Upstream' \
  -out domjudge/domjudge-tls.csr

cat > domjudge/domjudge-tls.ext <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=IP:$NATSUME_DOMJUDGE_IP
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF

openssl x509 -req -sha256 -days 365 \
  -in domjudge/domjudge-tls.csr \
  -CA public/local-origin-ca.crt -CAkey private/local-origin-ca-key.pem \
  -CAcreateserial -CAserial private/local-origin-ca.srl \
  -extfile domjudge/domjudge-tls.ext -out domjudge/domjudge-tls-leaf.pem

openssl verify -CAfile public/local-origin-ca.crt -purpose sslserver \
  -verify_ip "$NATSUME_DOMJUDGE_IP" domjudge/domjudge-tls-leaf.pem
openssl x509 -in domjudge/domjudge-tls-leaf.pem -noout -dates -ext subjectAltName
~~~

签发时输入既有 CA 私钥口令。有效期不得超过该 CA；若使用域名访问，SAN 改为 DNS:实际上游域名，并用 openssl verify -verify_hostname 核验。

将 domjudge-tls-leaf.pem 和 domjudge-tls-key.pem 部署到 DOMjudge 的 HTTPS 入口；叶子私钥未加密，须以受限权限保存。只交付这对叶子材料，不交付 CA 私钥。

若 HTTPS 入口是本机由 systemd 管理、master 进程以 root 运行的 Nginx，在 Server 管理员终端安装材料：

~~~bash
sudo install -d -o root -g root -m 0700 /etc/nginx/tls
sudo install -o root -g root -m 0644 \
  "$NATSUME_PKI_DIR/domjudge/domjudge-tls-leaf.pem" \
  /etc/nginx/tls/domjudge-tls-leaf.pem
sudo install -o root -g root -m 0600 \
  "$NATSUME_PKI_DIR/domjudge/domjudge-tls-key.pem" \
  /etc/nginx/tls/domjudge-tls-key.pem

sudo stat -c '%U:%G %a %n' /etc/nginx/tls \
  /etc/nginx/tls/domjudge-tls-leaf.pem \
  /etc/nginx/tls/domjudge-tls-key.pem
~~~

预期全部归属 root:root，目录 700、证书 644、私钥 600。Nginx 使用 PEM 编码的证书和私钥，可直接使用本节生成的 .pem 文件；见 [Nginx SSL 配置说明](https://nginx.org/en/docs/http/ngx_http_ssl_module.html#ssl_certificate)。

用 sudoedit 编辑已启用的 DOMjudge 站点配置，在现有 server 块中增加或调整以下 TLS 指令，保留原有的页面、PHP 和反向代理配置：

~~~nginx
listen 443 ssl;
ssl_certificate     /etc/nginx/tls/domjudge-tls-leaf.pem;
ssl_certificate_key /etc/nginx/tls/domjudge-tls-key.pem;
~~~

证书和私钥由 root master 进程加载，配置检查须使用 sudo 才能读取受限目录和私钥。检查成功后再重载已运行的服务：

~~~bash
sudo nginx -t && sudo systemctl reload nginx
~~~

若 sudo nginx -t 仍然报错，根据错误中的路径检查文件和各级目录的权限。容器或非 root master 部署按其实际挂载路径和运行用户设置读取权限。启用 HTTPS 后，从 Client 验证：

~~~bash
curl --fail --show-error --cacert /etc/natsume/trust/local-origin-ca.crt \
  https://192.0.2.20/
~~~

完成本节后用 exit 返回原管理员终端。随后将 Server config.toml 的 [runtime].domjudge_origin 设置为实际 HTTPS origin。第 7.3 节仍需把 Local Origin CA 加入 **Client 系统信任库**，并在信任库变更后重启 natsume-device-daemon.service，使 Caddy 重新加载信任。完成后再用不带 --cacert 的 curl 验证系统信任。

## 3. 整理本机部署目录并下载 Release

### 3.1 整理部署目录

**执行位置：Server，继续使用生成 PKI 的管理员账号。**建立 Git 仓库外受控目录：

~~~bash
umask 077
NATSUME_DEPLOY_DIR="$HOME/natsume-deploy"
mkdir -p "$NATSUME_DEPLOY_DIR"/{public,server,client,packages}
chmod 0700 "$NATSUME_DEPLOY_DIR" "$NATSUME_DEPLOY_DIR/server"
~~~

setup 成功后按第 4.5 节把公共交接材料放入 public/、client/；无需在此生成或复制服务私钥。手工部署使用附录 A.6 的完整清单。

### 3.2 下载并验证 Server 包

在 Server 本机从 [GitHub Releases](https://github.com/4o3F/Natsume/releases) 选定已发布版本。以下变量在后续构建步骤中保持一致：

~~~bash
export NATSUME_VERSION='2.10.3'
cd "$NATSUME_DEPLOY_DIR/packages"
NATSUME_RELEASE_URL="https://github.com/4o3F/Natsume/releases/download/v${NATSUME_VERSION}"

curl --fail --location --remote-name "$NATSUME_RELEASE_URL/natsume-server_${NATSUME_VERSION}_amd64.deb"
curl --fail --location --remote-name "$NATSUME_RELEASE_URL/SHA256SUMS"
test -s "natsume-server_${NATSUME_VERSION}_amd64.deb"
sha256sum --check --ignore-missing SHA256SUMS
dpkg-deb --field "natsume-server_${NATSUME_VERSION}_amd64.deb" Package Version Architecture
~~~

Server 校验须为 OK，版本与 `NATSUME_VERSION` 一致，架构为 amd64。当前 Release 的 SHA256SUMS 列出 Server、Client 和独立打印 wrapper 三种包，此处用 --ignore-missing 跳过未下载的其他包；Client 在第 6 节从相同标签构建。归档 Deb 和 checksum。

## 4. 运行 natsume-server-setup

### 4.1 安装软件包与预检

**执行位置：Server 管理员终端。**

~~~bash
dpkg --print-architecture
sudo apt-get update
sudo apt-get install --yes ca-certificates openssl curl sqlite3 python3
sudo apt-get install --yes "$NATSUME_DEPLOY_DIR/packages/natsume-server_${NATSUME_VERSION}_amd64.deb"
dpkg-query -W natsume-server
getent passwd natsume-server
~~~

预期架构 amd64、包已配置、服务账号存在。安装脚本初始化服务账号和目录等，不生成配置/CA、不初始化管理员、不自动启用或启动服务；缺少部署文件时允许通用包预装。

setup 还要求 root、stdin/stdout/stderr 均为 TTY、可用的 controlling terminal、运行中的 systemd、amd64 和 Deb 创建的服务账号/目录。预检显示当前 Server 时区，管理员须先用 `timedatectl status` 核对 VM 时区。时钟未同步、同步状态未知或时区未知时会警告并要求确认；应先修正时间和时区。不能重定向 stderr，其中包含提示和作答上下文；诊断请直接查看终端输出，不录制含密钥导出的终端会话。

~~~bash
sudo natsume-server-setup
~~~

需要自定义管理员 PKI 目录时，从首次运行起固定使用绝对路径：

~~~bash
sudo natsume-server-setup --pki-dir /root/contest-pki
~~~

默认 /root/natsume-pki，root:root 0700，与服务私钥目录分离；不支持应答文件。安装脚本只提示管理员显式运行工具，绝不调用它。

### 4.2 交互、计划确认与私钥保存

向导顺序是预检 → 第 1 节参数 → Control CA 来源 → TLS 来源 → Origin CA 来源 → PKI 目录说明与新 CA 口令 → 校验 → 计划确认。CSR 首轮在 TLS 来源后直接准备请求，不询问 Origin CA。计划列出参数、证书指纹/期限、路径、属主权限；确认前不发布部署文件。Esc 可在写入/私钥展示前返回支持的上一步，Ctrl-C 中止。

比赛结束和 Gateway 到期各分为日期面板与 `HH:MM` 时间输入；时间按 Server 本机时区换算为 UTC。每项完成后及计划摘要中均显示本地时间、时区名称、该时刻的 UTC 偏移和 UTC 结果，确认时逐项核对。比赛结束日期默认今天、时间无默认；Gateway 日期默认次日，时间默认相同。跨夏令时变更时仍须满足实际经过至少 86400 秒的规则；不存在或重复的本地时间会说明原因并重问时间。时间字段按 Esc 返回对应日期，日期字段按 Esc 返回前一参数。

确认后，每把新生成的 CA 私钥以各自口令加密的 PKCS#8 PEM **展示一次**；口令非空并输入两次，运维应使用强口令。先安全复制，再回答是否落盘；默认否，否时必须输入 key saved 确认另存，或 abort 取消。选是则以 root:root 0600 保存 control-ca-key.pem / origin-ca-key.pem；新生成 Control CA 的公共证书保存为 control-ca.crt（0644）。工具尝试清屏和清理回滚缓冲；不要录制含私钥的终端会话。导入的 CA 私钥原件不另存，Control CA 私钥永不进入服务目录。

完整安装写入 /etc/natsume-server/config.toml、两份 /etc/natsume/trust/*.crt（root:root 0644，父目录 0755），以及 keys 下 origin-ca.der、origin-ca-key.pk8、server-tls-leaf.der、server-tls-key.pk8（natsume-server:natsume-server 0600，keys 0700）。在线 key 为未加密 PKCS#8 DER，Origin DER 与公共 PEM 解码结果完全相同。写入顺序为 keys → trust → config，单文件原子发布且不覆盖已有文件。

生成配置显式写入 `[storage].journal = "/var/lib/natsume-server/natsume-journal.db"` 和 `[journal]` 的 Server 默认保留预算，与 `packaging/server/config.example.toml` 一致，不增加交互问题。Server 首次启动时以 `natsume-server` 服务用户创建日志库；setup 不直接创建、修改或删除业务库与日志库。安装后按第 8.5 节调整日志预算。

随后以服务用户 probe 复核部署并只读检测数据库。已初始化则跳过 bootstrap；否则以 natsume-server UID/GID 继承当前 TTY 调用 bootstrap，管理员凭据由 Server 直接读取，setup 不接收。失败先重新探测，再提示重试，最多三次。不要自行以 root 运行 bootstrap。

### 4.3 CSR 两阶段续接

离线 CA 签名返回 leaf 后，在同一 PKI 目录重跑：

~~~bash
sudo natsume-server-setup
# 自定义目录时仍使用同一个 --pki-dir
~~~

工具重新检查 pending 的权限、参数和指纹，询问已签证书路径并核对签发者、签名、key 和全部 IP；然后继续选择 Origin CA、严格校验和完整计划确认。无需重输参数或 Control CA。成功后只删除四个未改动的 pending 文件，目录为空则删除；额外文件保留。中途失败保留请求以便续接。要变更请求参数，先确认尚未部署，再由管理员有意作废并清理整个 pending 后重新生成；不要手改 request.toml 或替换 key。

### 4.4 systemd、健康检查与失败重跑

setup 执行 daemon-reload、enable、start，并对首个 Server IP 的 /api/v2/health 进行唯一 Control CA 信任的 TLS 1.3 / HTTP/1.1 检查，不用代理或重定向；每次超时 5 秒、响应上限 4 KiB，最多 20 次，间隔 1.5 秒。要求 HTTP 200 且 JSON 仅包含 status: ok。这是服务存活检查；DOMjudge、Client 网络与桌面仍按后文验收。

**提交点为健康检查通过。**此前普通失败会尝试停服、在本次已 enable 时 disable，删除本次创建且未变动的配置/trust/在线材料和本次落盘 CA 备份，清理新建空目录。已变动文件、不空目录及清理失败会报告；业务库 `natsume.db`、日志库 `natsume-journal.db`、两库的 `-wal`／`-shm` 和 `keys/server-root.key` 永不删除。Deb 通过 tmpfiles 提供的 state 目录也保留，即使它在规划后由本次 apply 重建。bootstrap 完成或服务曾启动后回滚，这些持久文件均不参与已部署检测，重跑仍可继续，已完成的初始化会跳过；日志预算调整见第 8.5 节。已展示的 CA 私钥必须作废，重试生成新钥。

若任意七个部署路径存在（config、两份 trust、四份 TLS/Origin 文件），setup 拒绝覆盖；已部署变更见第 8.4 节。断电或强制终止不保证整组回滚，须先检查残留，不能直接删除数据库或 key 强行重装。

退出码：0 表示已安装或 CSR 等待签名；1 表示失败；2 表示交互中止或 CLI 参数错误；中止后也须检查回滚诊断，退出 2 不代表清理全部成功。健康检查后的交接导出、摘要输出或 pending 清理失败只记日志，仍保留服务且退出 0；须按日志补齐交接或清理。可核对：

~~~bash
sudo systemctl status natsume-server.service --no-pager
sudo journalctl -u natsume-server.service -b -n 100 --no-pager
~~~

### 4.5 使用 Client 交接材料

成功后摘要给出两份 CA 指纹/有效期、私钥去向，以及 /root/natsume-pki/handoff/<UTC 时间戳>/。目录包含 config.toml、control-ca.crt、local-origin-ca.crt 和 SHA256SUMS，文件 root:root 0644；PKI 父目录仍 0700。只复制该公共目录的文件到交接区，不交付整个 PKI 目录：

~~~bash
NATSUME_HANDOFF_DIR='/root/natsume-pki/handoff/20261028T120000Z'
sudo sh -c 'cd "$1" && sha256sum --check SHA256SUMS' sh "$NATSUME_HANDOFF_DIR"
sudo install -o "$(id -un)" -g "$(id -gn)" -m 0644 \
  "$NATSUME_HANDOFF_DIR/config.toml" "$NATSUME_DEPLOY_DIR/client/config.toml"
sudo install -o "$(id -un)" -g "$(id -gn)" -m 0644 \
  "$NATSUME_HANDOFF_DIR/control-ca.crt" "$NATSUME_HANDOFF_DIR/local-origin-ca.crt" \
  "$NATSUME_DEPLOY_DIR/public/"
~~~

将时间戳替换为摘要实际值，自定义目录也同步替换。通过独立可信渠道核对指纹，再按第 7 节交接给 Client 部署方。交接导出失败时按第 7.1 节手工生成首个 IP 对应的 Client 配置，从 /etc/natsume/trust 复制公共证书并核对摘要指纹；无需重新运行 setup。

## 5. 管理入口与比赛数据

### 5.1 信任 Control CA，登录 Panel

在用于访问 Panel 的管理员 Firefox 中：设置 → 隐私与安全 → 证书 → 查看证书 → 证书颁发机构 → 导入本机生成、经过指纹核对的 control-ca.crt，允许识别网站。访问 https://实际ServerIP:8443/，应无证书警告，使用 bootstrap 管理员登录。

### 5.2 导入完整队伍名单

当前版本使用完整 XLSX 名单导入。升级已有系统前，先阅读所选 Release 的迁移说明并完成第 8.2 节的一致备份。

在 Web **Preparation** 点击 **Download Excel template**，下载 `Teams` 工作表模板。人工把报名表整理为一行一队，按[字段规则](../crates/natsume-server/docs/roster.md#excel-模板)填写九列：学校中英文名、country、account、password、seat、队伍中英文名、category。所有单元格按文本填写，保留座位前导零；不填写公式。学校与队伍的中英文名各至少填写一种，country 留空默认为 CHN。

填写 DOMjudge 队伍账号和当前密码；account、seat 各自唯一。学校编号由 Server 生成，无需预填。文件包含密码，在 Server 受控目录保存时设为 0600，不提交 Git。每次上传完整名单；热身转正式赛只修改 password，其余各行也一并上传。

1. 选择 XLSX（最大 8 MiB），点击 **Create preview**。
2. 核对 **Team changes / School changes / Preview school logos**，检查队伍、中英文名称、类别和 INST ID。
3. 核对账号新增／移除、**Passwords changed**、**Seat changes / Mapping changes** 和 **Binding impacts**。这里只显示密码变化的账号名。
4. 删除已绑定的座位会阻止提交，需要先通过 **Bindings** 解绑；其他资料变化会列出受影响设备，绑定保留在原座位。
5. 点击 **Commit import** → **Confirm commit**。整份名单原子生效；完全相同的文件不改业务数据，只有实际改密账号推进凭据 revision。队伍资料变化不切换设备前台会话。
6. 在 **Seats / Accounts** 核对数量与映射。刷新或退出后需要 **Discard preview** 再上传；普通页面内导航仍保留审核过的文件。

当前版本提供 Logo 目录、网页观测和完整 DOMjudge ZIP。配套 Client 在 waiting 页面显示队伍／学校／Logo，断网时保留上次资料并显示离线标识。

在同一份 Server config.toml 的 `[storage]` 中设置 `organization_logos`（示例已列出）。Server Deb 在安装／重装时自动创建默认目录 `/var/lib/natsume-server/organization-logos`，归属 `root:natsume-server`、权限 `0750`，保留已有图片。自定义路径由部署方创建并授予服务用户读取／遍历权限。首次配置路径需要重启 Server；以后补图或替换源文件无需重启或重新导入名单。

`[storage].print_documents` 是必填绝对路径。Server Deb 创建默认目录 `/var/lib/natsume-server/print-documents`，归属 `natsume-server:natsume-server`、权限 `0700`，PDF 文件权限为 `0600`。自定义路径需要保护父目录、授予服务用户读写与遍历权限，并按需调整 systemd 的 `ReadWritePaths`；Server 仅在父目录已存在时创建末级目录，拒绝末级符号链接、不安全 owner 或组／其他用户可写目录。该目录与数据库共同构成持久状态，不可在 Server 运行期间移动、替换或清理。

**执行位置：Server 管理员终端。**复制已按完整学校中文名或英文名命名的源图，设置为 `root:natsume-server`、`0640`；下面的 `./organization-logos/` 是部署方已准备好的本地目录：

```bash
sudo find ./organization-logos -maxdepth 1 -type f \
  -exec install -o root -g natsume-server -m 0640 -t \
  /var/lib/natsume-server/organization-logos -- {} +
sudo -u natsume-server -- test -r /var/lib/natsume-server/organization-logos
sudo -u natsume-server -- test -x /var/lib/natsume-server/organization-logos
```

候选后缀为 PNG/JPG/JPEG/WebP/SVG（大小写均可）；内容实际为 PNG 或 SVG 的 `.webp` 文件也能使用。匹配必须唯一：同一学校不要同时留中文名／英文名两份或多种格式。只读取直接子文件，拒绝符号链接。单文件最多 8 MiB、每边最多 4096 像素、总计最多 4194304 像素。SVG 不读取外部资源；含文字的 SVG 需要 Server 安装对应字体，推荐将文字转路径以固定效果。可先按共享库文档运行离线预检。

在 Preparation Center 核对 **Preview school logos** 或 **Committed roster & DOMjudge export** 的学校 ID、缩略图、源文件与状态。用搜索、**Problems** 筛选缺失／歧义／损坏，用 **Refresh logos** 重新读取目录；刷新保留表格滚动位置。原图按校名保存，不需要自行生成 INST 命名副本。替换单图时先写入不参与匹配的临时文件，再在同目录原子重命名覆盖，避免请求恰好读到半写入文件。

提交名单后，管理员点击 **Download DOMjudge ZIP**，导出当前已提交的完整名单，独立于待审预览、设备分页和绑定状态。ZIP 包含：

- `groups.json`、`organizations.json`、`teams.json`、`accounts.yaml`；
- `logos/INST-xxx.png`（当前名单所有可用校徽，按实际内容转为 PNG）。

DOMjudge 使用新版 JSON／YAML 入口按 groups → organizations → teams → accounts 顺序导入；将 `logos/` 内容复制到 DOMjudge 的 `webapp/public/images/affiliations/`，保留 INST 文件名并赋予 Web 服务读权限。不使用 legacy TSV。缺图／歧义不会进入导出且不阻塞；匹配到的图片损坏、不可读或超限则整包失败，页面显示学校和文件原因。

ZIP 包含当前明文密码，应保存在受控目录；Server 不落地保存，浏览器正常下载，不写入 localStorage/sessionStorage。密码切换后重新导出并在 DOMjudge 导入 accounts，实际测试队伍登录；Natsume 不会自动修改 DOMjudge。数据库／vault 和源图片目录需要分别备份。

### 5.3 检查注册窗口（可选 API 会话）

Web **Enrollment** 的 **Enrollment window** 显示当前注册窗口状态，admin 可以点击 **Open window / Close window**，viewer 只能查看。**Open** 自动批准新请求及当前在线的待审请求；**Closed** 保留请求等待 admin 批准或拒绝。Server 启动后窗口为 **Closed**；批量部署时可以开启自动审批。

通过 Web 操作即可完成后续注册流程。若需从终端操作，也可以建立以下 API 会话；窗口使用 GET/PUT /api/v2/provisioning-window，PUT 需要 admin。

**执行位置：Server 管理员终端。**密码从 TTY 读取，不放入命令历史：

~~~bash
NATSUME_SERVER_ORIGIN='https://192.0.2.10:8443'
NATSUME_CONTROL_CA="$HOME/natsume-deploy/public/control-ca.crt"
NATSUME_API_DIR="$(mktemp -d)"
chmod 0700 "$NATSUME_API_DIR"

python3 - "$NATSUME_API_DIR/login.json" <<'PY'
import getpass
import json
import os
import sys

with open('/dev/tty') as tty:
    print('Operator login name: ', end='', file=sys.stderr, flush=True)
    name = tty.readline().rstrip('\n')
password = getpass.getpass('Operator password: ')
fd = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as f:
    json.dump({'login_name': name, 'password': password}, f)
PY

curl --fail-with-body --silent --show-error --http1.1 \
  --cacert "$NATSUME_CONTROL_CA" --cookie-jar "$NATSUME_API_DIR/cookies" \
  --header 'Content-Type: application/json' \
  --data-binary "@$NATSUME_API_DIR/login.json" \
  "$NATSUME_SERVER_ORIGIN/api/v2/session"
rm "$NATSUME_API_DIR/login.json"

curl --fail-with-body --silent --show-error --http1.1 \
  --cacert "$NATSUME_CONTROL_CA" --cookie "$NATSUME_API_DIR/cookies" \
  "$NATSUME_SERVER_ORIGIN/api/v2/provisioning-window"
~~~

登录响应应为 admin，初始窗口 closed。保留变量和受限临时目录供 7.5 节使用；cookie 是凭据，401 时重新登录。安装好要注册的机器后再开窗口。

## 6. 从源码构建 Client Deb

### 6.1 安装编译依赖和固定 Rust 工具链

**执行位置：同一台 Ubuntu 24.04 amd64 Server，使用管理员账号。**在本机安装 Rust 和仓库 `.deno-version` 指定的 Deno，只构建 Client 的三个 Rust 程序和 Deb，无需 Node/pnpm/Web 编译环境。源码放在管理员 Home 的独立目录，不放入 PKI 或部署材料目录：

~~~bash
sudo apt-get update
sudo apt-get install --yes build-essential pkg-config curl git ca-certificates \
  libfontconfig1-dev libgl1-mesa-dev libudev-dev tar xz-utils
mkdir -p "$HOME/src"
cd "$HOME/src"
git clone --branch "v${NATSUME_VERSION}" --depth 1 https://github.com/4o3F/Natsume.git Natsume
cd Natsume
git rev-parse HEAD
~~~

按 [Rust 官方安装说明](https://rust-lang.org/tools/install/)安装 rustup。以下先下载并审阅安装器，再安装仓库声明的 Rust 1.97.1：

~~~bash
curl --fail --location https://sh.rustup.rs --output /tmp/natsume-rustup-init.sh
less /tmp/natsume-rustup-init.sh
sh /tmp/natsume-rustup-init.sh --profile minimal --default-toolchain 1.97.1
. "$HOME/.cargo/env"
rustup show active-toolchain
cargo --version
~~~

如果已经安装 rustup，改为运行 rustup toolchain install 1.97.1 --profile minimal；进入仓库后 rust-toolchain.toml 会选择对应版本。不要通过修改 lockfile 适应其他工具链。

按 [Deno 安装说明](https://docs.deno.com/runtime/getting_started/installation/)安装稳定版 Deno 2.9.8 或更新的 2.x 版本。`.deno-version` 固定 CI 验证版本并定义本地最低版本，同一主版本的更新稳定版无需降级；新主版本须另行验证。后续命令在仓库根目录同一终端运行，保留本次 Git SHA。Rust/Slint release 编译需要足够磁盘和内存；需要限制并发时可设置 `CARGO_BUILD_JOBS=2`。

### 6.2 准备构建依赖、工具与资产

prepare 负责下载锁定的 Deno 模块、Cargo 依赖、Skia 预编译库、Caddy、nFPM 和公开作者头像。Caddy／nFPM 使用仓库现有固定版本和 SHA；固定缓存损坏或头像下载失败会中止，不静默替换为其他版本。

~~~bash
deno task prepare client
~~~

工具和下载缓存在 `target/build/`。只有 prepare 下载输入；后续 build／package 使用缓存，不隐式安装依赖。依赖或工具清单改变后重新 prepare。Client 专用准备不运行 pnpm，也不下载打印资产。

### 6.3 编译 Client 三个程序

使用独立 target 目录，避免误取旧构建产物：

~~~bash
export CARGO_TARGET_DIR="$PWD/target/client-deb"
deno task build client

ls -lh "$CARGO_TARGET_DIR/release/natsume-device-daemon" \
  "$CARGO_TARGET_DIR/release/natsume-privileged-helper" \
  "$CARGO_TARGET_DIR/release/natsume-session-agent"
~~~

使用默认生产特性构建，不增加 --all-features 或测试特性；构建输入仅包含源码和已准备的依赖。

### 6.4 使用现有 manifest 打包

下面使用仓库 TypeScript 打包入口与原有 nFPM manifest，不要求全局安装 nFPM。源码构建使用可区分的本地包版本：

~~~bash
export NATSUME_CLIENT_VERSION="${NATSUME_VERSION}~local1"
export VERSION="$NATSUME_CLIENT_VERSION"
export RUST_RELEASE_DIR="$CARGO_TARGET_DIR/release"
deno task package client

(
  cd dist/packages
  sha256sum "natsume-client_${NATSUME_CLIENT_VERSION}_amd64.deb" > "natsume-client_${NATSUME_CLIENT_VERSION}_amd64.deb.sha256"
)
~~~

交付产物为 `dist/packages/natsume-client_${NATSUME_CLIENT_VERSION}_amd64.deb` 及本次生成的 checksum；示例版本为 `2.10.3~local1`。记录并交接 `NATSUME_CLIENT_VERSION`，第 7.2 节使用同一值。package 使用已构建的程序和准备好的资产。

通用 Deb 包含程序、Caddy、包所属运行文件及配置示例；部署配置和 CA 由第 7 节单独准备。

自建包摘要不一定与官方 Release 相同，应配套使用**本次生成的 checksum**。如果改过源码，选择有区别的 Debian 版本，同步命令中的文件名，记录 Git SHA、改动、工具版本和日志，不冒充官方同名产物。

## 7. Client 部署输入与首次联调

本节交接构建产物，并在**已完成配套桌面集成的样机**上联调。

### 7.1 准备包外输入并交接

setup 安装已按第 4.5 节复制完整 Client 配置和两份公共 CA，可直接使用下表交接。手工部署或交接导出失败时，在 Server 本机创建 Client config.toml，替换实际首个 IP、端口和 Gateway hostname，并从 /etc/natsume/trust 复制两份公共 CA 到部署目录 public/：

~~~bash
NATSUME_DEPLOY_DIR="$HOME/natsume-deploy"
cat > "$NATSUME_DEPLOY_DIR/client/config.toml" <<'EOF'
[server]
ip = "192.0.2.10"
port = 8443

[site]
gateway_hostname = "domjudge"
EOF
~~~

交给 Client 部署方：

| 输入 | 最终路径 / 用途 |
| --- | --- |
| 自建 Client Deb 和对应 checksum | 安装通用 Client |
| client/config.toml | /etc/natsume/config.toml |
| public/control-ca.crt | /etc/natsume/trust/control-ca.crt |
| public/local-origin-ca.crt | /etc/natsume/trust/local-origin-ca.crt |
| 可选 DOMjudge 上游公共 CA | 系统信任，供 Caddy 校验私有上游 |

不交付 Server 私钥、CA 私钥、设备身份或 token。

若部署方先预装 Deb，可稍后再提供配置与两个 CA；首次启动前必须补齐。包脚本只做存在性/非空/可读等基础检查，不生成或改写这些文件。

### 7.2 在配套 Client 样机安装并落地配置

把 Deb、它的 checksum、config.toml、两个公共 CA 放到样机管理员的 ~/natsume-client-install/。先校验包，然后安装；文件落地前保持 Daemon 停止：

~~~bash
cd "$HOME/natsume-client-install"
# 从交接清单填写第 6.4 节实际构建的版本。
NATSUME_CLIENT_VERSION='2.10.3~local1'
sha256sum --check "natsume-client_${NATSUME_CLIENT_VERSION}_amd64.deb.sha256"
sudo apt-get update
sudo apt-get install --yes "$PWD/natsume-client_${NATSUME_CLIENT_VERSION}_amd64.deb"

sudo install -d -o root -g root -m 0755 /etc/natsume /etc/natsume/trust
sudo install -o root -g root -m 0644 config.toml /etc/natsume/config.toml
sudo install -o root -g root -m 0644 control-ca.crt /etc/natsume/trust/control-ca.crt
sudo install -o root -g root -m 0644 local-origin-ca.crt /etc/natsume/trust/local-origin-ca.crt
~~~

检查配置和证书指纹与 Server 部署一致。Client 启动时由 Device Daemon 从配置解析 `[site].gateway_hostname` 并传给 Privileged Helper，后者校验域名后自动维护 `/etc/hosts` 中的 `BEGIN NATSUME GATEWAY` / `END NATSUME GATEWAY` 区块，将该名称映射到 127.0.0.1 和 ::1，并同步 Firefox 的主页、`Contest Site` 工具栏书签及已有的相关通知许可。

升级到包含本机 Gateway 配置能力的 Client 后，先执行 `sudo systemctl restart natsume-privileged-helper.service natsume-device-daemon.service`，确保两个运行进程都加载配套二进制。

修改 Gateway hostname 后，配套更新 Server 配置，再在 Client 执行 `sudo systemctl restart natsume-device-daemon.service`；完全退出再打开 Firefox。Natsume 会删除旧的受管映射、迁移自己的浏览器入口，保留其他 hosts 别名、Firefox 策略和 CA 设置。重复启动不重写未变内容，Home reset 不影响这些 `/etc` 设置。此同步不依赖注册审批或网络连接。

`/etc/hosts`、已有的 `/etc/firefox/policies/policies.json` 及父目录须为 root 所有的普通文件/真实目录，仅允许 root 用户/组写入，不能使用符号链接。Firefox 策略不存在时自动以 root:root 0644 创建，目录为 0755。损坏的 TOML、JSON 或 Natsume hosts 区块会使启动失败，日志包含具体路径与原因；修复后重启 Client，不删除身份或 Home 状态。

### 7.3 系统与 Firefox 信任

为 Local Origin CA 配置系统信任，在已有 Firefox 策略中追加证书，保留其余配置：

~~~bash
sudo ln -sfn /etc/natsume/trust/local-origin-ca.crt \
  /usr/local/share/ca-certificates/natsume-local-origin-ca.crt
sudo update-ca-certificates
sudo python3 - <<'PY'
import json
from pathlib import Path

path = Path('/etc/firefox/policies/policies.json')
data = json.loads(path.read_text()) if path.exists() else {'policies': {}}
certs = data.setdefault('policies', {}).setdefault('Certificates', {})
files = certs.setdefault('Install', [])
ca = '/etc/natsume/trust/local-origin-ca.crt'
if ca not in files:
    files.append(ca)
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps(data, indent=2) + '\n')
path.chmod(0o644)
PY
~~~

Linux Firefox 不能只靠 ImportEnterpriseRoots=true 导入系统 CA；[Mozilla 官方说明](https://firefox-admin-docs.mozilla.org/reference/policies/certificates/)中该开关仅支持 Windows/macOS，Linux 可用 Certificates.Install 的绝对路径导入 PEM/DER。

上述路径适用于配套 Deb Firefox/Firefox ESR，换用 Snap 等发行形式需重验路径和访问能力。完全退出再启动 Firefox，通过 about:policies 的 Active/Errors 及证书颁发机构列表核对。

若按 2.7 节复用 Local Origin CA，以上系统信任步骤同时覆盖 DOMjudge 上游。若使用另一份私有 CA，将其公共 PEM 以 root:root 0644 安装至 /usr/local/share/ca-certificates/domjudge-upstream-ca.crt，执行 sudo update-ca-certificates。信任库变更后重启 natsume-device-daemon.service，再核对上游访问。

### 7.4 启动并检查配套样机

确认桌面集成、waiting/teams 和 Home 模板已完成后：

~~~bash
dpkg-query -W natsume-client
getent passwd waiting teams
findmnt /usr/lib/natsume/home-templates/current/lower
sudo systemctl enable --now natsume-privileged-helper.service natsume-device-daemon.service
systemctl status natsume-privileged-helper.service natsume-device-daemon.service --no-pager
sudo journalctl -u natsume-privileged-helper.service -u natsume-device-daemon.service -b -n 150 --no-pager
getent ahostsv4 domjudge
getent hosts domjudge

curl --fail --show-error --http1.1 --tlsv1.3 \
  --cacert /etc/natsume/trust/control-ca.crt \
  https://192.0.2.10:8443/api/v2/health
curl --show-error --silent --output /dev/null --write-out '%{http_code}\n' \
  https://judge.contest.example/
~~~

以上 getent 的 domjudge 替换为实际 `[site].gateway_hostname`，在日志出现 `local Gateway settings synchronized` 后检查。Firefox 重新打开后，在 `about:policies` 的 Active 中确认 Homepage.URL 和 Contest Site 书签使用同一个域名，Errors 为空。

预期包版本与交接清单一致、模板只读 SquashFS、Gateway 仅解析到 loopback，Server 和上游 TLS/HTTP 正常。第二个 curl 使用系统信任，核对 Caddy 的上游信任来源。Caddy 由 Daemon 依赖管理，Agent 由官方 Kiosk 用户服务管理，不单独增加另一个启动入口。

### 7.5 注册审批与绑定

批量自动注册时，在 Web **Enrollment → Enrollment window** 点击 **Open window**，等待状态变为 **Open**。Server 会自动批准新请求和当前在线的待审请求；随后在 **Devices** 检查注册结果。若提交失败，先根据页面错误排查。

也可在 **Server 管理员终端，使用 5.3 节变量与管理员 cookie** 执行：

~~~bash
curl --fail-with-body --silent --show-error --http1.1 \
  --cacert "$NATSUME_CONTROL_CA" --cookie "$NATSUME_API_DIR/cookies" \
  --request PUT --header 'Content-Type: application/json' \
  --data '{"state":"open"}' \
  "$NATSUME_SERVER_ORIGIN/api/v2/provisioning-window"
~~~

API 预期 state=open，此时不需要逐台点击 **Approve**。

需要逐台人工审核时，让窗口保持 **Closed**。Web **Enrollment** 仍会出现 pending review；逐台核对物理工位、硬件 ID、证据质量、Daemon/Agent 版本和候选公钥，再点 **Approve**；未知设备使用 **Deny** 并排查。

Server 每次重启窗口恢复关闭，新请求改为人工审批。Client 完成身份初始化和连接后才开始注册。

样机显示 **Bind workstation / Enter your seat code** 时输入 XLSX 中的工位码，如 A-01。在 **Bindings / Seats / Devices** 核对工位、设备和账号一致。工位不存在/被占时修正映射，不删除设备身份文件重试。

新设备默认保持 waiting，绑定成功后等待管理员执行 **Show contest desktop**。绑定和重启不会改写已有的前台目标；已有设备若要继续等待，先在 **Targets** 执行 **Show waiting screen**。重启先进入 waiting，重新连接且桌面依赖就绪后按 Server 保存的目标恢复前台。

### 7.6 验收实际桌面和 DOMjudge

**Targets** 选样机 → **Show contest desktop**，通过当前报告确认收敛：

| 项目 | 验收要求 |
| --- | --- |
| Connection | 已连接且有新报告，不是 offline / awaiting fresh state |
| Gateway | 目标与实际一致，可服务而非 BLOCKED |
| Binding | 正确工位/账号，凭据已应用 |
| Runtime | 实际上游 origin 与部署值一致 |
| Session | 实际前台 contest，contest_ready=true |
| Home | 模板/Home 正常，无 recovery_required |

Client 检查：

~~~bash
sudo ss -lntp 'sport = :443'
curl --show-error --silent --output /dev/null --write-out '%{http_code}\n' \
  --cacert /etc/natsume/trust/local-origin-ca.crt \
  --resolve domjudge:443:127.0.0.1 https://domjudge/
~~~

仅监听 127.0.0.1:443 和 [::1]:443，TLS 和业务响应正常。

在 **teams 比赛桌面的 Firefox** 访问 https://domjudge/，必要时进入 /login：

1. 无证书警告，地址仍为 Gateway hostname。
2. 登录显示绑定工位的正确 DOMjudge 账号。
3. 页面、静态资源、提交题目和所需实时更新正常。
4. 无跳转绕开 Gateway、跨工位串号等问题。

同时验收真实 DOMjudge 的登录头、重定向、Cookie、Host 和资源行为。不要把管理员浏览器 profile 制作成比赛模板。

测试 **Show waiting screen** → **Show contest desktop**，确认前台切换及比赛进程/Home 保留。在比赛终端运行 `echo "$XDG_SESSION_TYPE"` 应为 `wayland`。

**Terminate** 终止并重建比赛会话，**Reset home** 重置比赛 Home。只在授权样机/维护窗口测试，先保存数据，不用在赛工位做破坏性验证。

### 7.7 完成后关闭窗口，注销会话

在 Web **Enrollment → Enrollment window** 点击 **Close window**，确认状态变为 **Closed** 后点击 **Logout**。

如果建立了 5.3 节的 API 会话，回到 Server 管理员终端，关闭窗口并注销、清理该会话：

~~~bash
curl --fail-with-body --silent --show-error --http1.1 \
  --cacert "$NATSUME_CONTROL_CA" --cookie "$NATSUME_API_DIR/cookies" \
  --request PUT --header 'Content-Type: application/json' \
  --data '{"state":"closed"}' \
  "$NATSUME_SERVER_ORIGIN/api/v2/provisioning-window"

curl --fail-with-body --silent --show-error --http1.1 \
  --cacert "$NATSUME_CONTROL_CA" --cookie "$NATSUME_API_DIR/cookies" \
  --request DELETE "$NATSUME_SERVER_ORIGIN/api/v2/session"
rm "$NATSUME_API_DIR/cookies"
rmdir "$NATSUME_API_DIR"
~~~

归档源码版本、自建包 checksum、公共配置/CA 指纹、工位—硬件 ID—设备 ID 对照、验收结果和 Server 冷备份；账号 XLSX 和私有材料按凭据保管。

## 8. 日常维护、备份和恢复

### 8.1 日常检查

Server 检查：

~~~bash
systemctl is-active natsume-server.service
sudo journalctl -u natsume-server.service --since '1 hour ago' --no-pager
df -h /var/lib/natsume-server
openssl x509 -in /etc/natsume/trust/control-ca.crt -noout -dates
openssl x509 -in /etc/natsume/trust/local-origin-ca.crt -noout -dates
sudo openssl x509 -inform DER \
  -in /var/lib/natsume-server/keys/server-tls-leaf.der -noout -dates
~~~

Web 检查各设备实际状态和报告时间。不要公开原始数据库、cookie、私钥或运行中的 Caddy 配置，READY 配置包含访问凭据。tracing/OTLP 用于运行诊断；Events 和设备 Timeline 提供有损的 operational event journal，不构成无损审计账本。日志健康、预算和独立备份见第 8.5 节。

### 8.2 Server 冷备份

维护窗口停止唯一 Server 进程，确认没有手动运行的 serve 后，获取 SQLite 一致副本，再与同一时间点的 PDF 目录及 key/config 一起备份。整个备份期间保持停服。目标目录不得放在被归档的状态目录内；下例使用默认 PDF 目录，自定义配置必须将 `NATSUME_PRINT_DOCUMENTS_DIR` 改为 `[storage].print_documents` 的实际值：

~~~bash
sudo systemctl stop natsume-server.service
systemctl is-active natsume-server.service
~~~

预期 inactive（该命令非零退出）。确认后继续：

~~~bash
NATSUME_BACKUP_DIR="/var/backups/natsume/$(date -u +%Y%m%dT%H%M%SZ)"
NATSUME_PKI_DIR='/root/natsume-pki' # 使用安装时的实际 PKI 目录（或 --pki-dir）
NATSUME_PRINT_DOCUMENTS_DIR="/var/lib/natsume-server/print-documents"
sudo install -d -o root -g natsume-server -m 0710 /var/backups/natsume
sudo install -d -o natsume-server -g natsume-server -m 0700 "$NATSUME_BACKUP_DIR"
sudo -u natsume-server sqlite3 /var/lib/natsume-server/natsume.db \
  ".backup '$NATSUME_BACKUP_DIR/natsume.db'"
sudo chown root:root "$NATSUME_BACKUP_DIR" "$NATSUME_BACKUP_DIR/natsume.db"
sudo chmod 0600 "$NATSUME_BACKUP_DIR/natsume.db"
sudo install -o root -g root -m 0600 /dev/null "$NATSUME_BACKUP_DIR/config-and-keys.tar"
sudo tar --numeric-owner --acls --xattrs -cpf "$NATSUME_BACKUP_DIR/config-and-keys.tar" -C / \
  etc/natsume-server etc/natsume/trust var/lib/natsume-server/keys
sudo install -o root -g root -m 0600 /dev/null "$NATSUME_BACKUP_DIR/print-documents.tar"
sudo tar --numeric-owner --acls --xattrs -cpf "$NATSUME_BACKUP_DIR/print-documents.tar" \
  -C "$NATSUME_PRINT_DOCUMENTS_DIR" .
sudo install -o root -g root -m 0600 /dev/null "$NATSUME_BACKUP_DIR/pki.tar"
sudo tar -cpf "$NATSUME_BACKUP_DIR/pki.tar" -C "$NATSUME_PKI_DIR" .
sudo sqlite3 "$NATSUME_BACKUP_DIR/natsume.db" 'PRAGMA integrity_check;'
sudo tar -tf "$NATSUME_BACKUP_DIR/config-and-keys.tar"
sudo systemctl start natsume-server.service
~~~

完整性应为 ok。源数据库由服务用户打开，避免产生 root 所有的运行状态文件；备份完成后目录和文件归 root 管理。归档包含 DB 一致副本、PDF 目录、vault root、Origin CA、TLS 和完整公共配置，缺一不可。使用 SQLite .backup 避免仅复制主库漏掉 WAL，依据 [SQLite Online Backup](https://www.sqlite.org/backup.html)。

这里的 DB 是业务库。`natsume-journal.db` 是独立的可选诊断历史，按第 8.5 节另做备份；两库不构成同一事务快照，业务恢复不要求恢复日志库。

`NATSUME_PKI_DIR` 必须是安装时的 PKI 目录，默认 `/root/natsume-pki`；使用过 `--pki-dir` 则填入当时路径。`pki.tar` 保存该目录实际存在的内容，包括显式落盘的加密 CA 私钥备份、证书和签发记录；`handoff/` 只有公共交接材料，不能代替私钥备份。未选择落盘、由管理员另行保存的加密导出，以及导入 CA 的原件，须由管理员分别纳入备份并验证可解密。离线 Control CA 私钥与签发记录由离线 CA 保管方负责备份，本机归档不包含它们。备份期间暂停 PKI 签发操作；私钥口令与归档分开保管。

用现场备份系统加密复制到独立存储，同时保存匹配 Deb。server-root.key 丢失等于 vault 秘密不可恢复，只备份数据库或重新 bootstrap 一个 key 都不能恢复旧凭据。

### 8.3 恢复与管理员密码重置

恢复先在隔离环境演练，避免两台 Server 同时服务同一批 Client：

1. 安装相同版本 Deb，停服并保存故障现场快照。
2. 将 config-and-keys.tar 解包到受限 staging 目录，核对路径和材料。
3. 用同一备份集的 DB、print-documents.tar 和完整旧 key/config 替换目标；PDF 归档恢复到配置的 `storage.print_documents` 目录，不能与其他时间点的文件混用。不能混合新旧数据库、WAL、SHM；保留故障副本后移走旧 DB 配套文件。
4. 换主机时按目标服务 UID/GID 重设状态文件 owner，keys 和 PDF 目录 0700、秘密文件和 PDF 0600、公共配置和 CA root:root 0644。
5. 检查数据库完整性和证书匹配。Server 启动会逐份核对已入队 PDF 的长度和摘要，缺失或损坏时拒绝启动；核对通过后，按 4.5、7.6 节验收已注册设备重连。**不要运行 bootstrap。**

pki.tar 恢复到安装时的 PKI 目录（默认 /root/natsume-pki 或原 --pki-dir），保持 root:root、目录 0700、私钥文件 0600，供后续签发使用；它不解包到 natsume-server 服务私有目录。管理员另存的副本和离线 Control CA 备份按各自保管流程恢复。

只重置管理员密码时，在可信 TTY 和维护窗口运行：

~~~bash
sudo systemctl stop natsume-server.service
sudo -u natsume-server -- /usr/bin/natsume-server reset-operator-password
sudo systemctl start natsume-server.service
~~~

输入目标登录名和两次新密码。该管理员已有会话失效，不创建账号、不改 vault root。不要直接编辑密码 hash。

### 8.4 配置、证书和版本更新

本节仍为手工运维流程，natsume-server-setup 只做首次安装，不用于重配置、续签、替换 CA、升级或修复。不要删除部署输入来绕过已部署检测；先备份，再按下表操作。

| 变更 | 操作与验收 |
| --- | --- |
| 同一 Control CA 续签 Server leaf | 在 Server 管理员的 PKI 目录签发、核对 SAN/期限/公钥；停服替换 leaf/key，保持权限；重启并从 Client 验证 |
| 更换 Server IP | 先准备覆盖新 IP 的 leaf，再改 Client 配置/网络，重启 Daemon 并验收 |
| 修改 Gateway hostname | 两端配置、hosts、主页/书签与新 Gateway leaf 配套迁移，不承诺只改 TOML 就更新旧 leaf |
| 延长比赛或 Gateway 期限 | 核对 CA/Server leaf，更新配置；既有持久化 Gateway leaf 不保证自动重签，逐设备核对并安排凭据迁移 |
| 轮换任一 CA | 当前不是多根无缝轮换方案；安排停机，两端/浏览器/设备凭据配套迁移 |
| 修改 DOMjudge 上游 | 备份后修改 Server config.toml 的 [runtime].domjudge_origin，重启服务，核对 Runtime/Gateway |
| 升级 Deb | 先读迁移说明并完整备份；升级 Server 后显式重启；Client 与已应用的镜像/PAM/GDM/Home 模板共同验收 |

包不生成、覆盖、改权限或删除部署方 config/CA，更新文件由部署方负责。配置在进程启动时重新加载，不依赖安装脚本自动修正。

Client 升级不会自动修改镜像项目维护的桌面配置或 Home 模板。升级/回退须遵守[运行环境的维护要求](client-environment.zh-CN.md#维护要求)，不能只降级 Deb 然后沿用不兼容状态。不要克隆运行过机器的身份、Control/Gateway key、Enrollment 或 Home reset 状态到其他机器。

旧版使用站点 UUID 的 Client 不能通过删除配置项完成原地升级：本版的硬件 ID 派生规则和 identity.json 格式均已改变。维护时先完成 Home 恢复、退出受管会话并备份所需数据，再在 Server 解除旧 Binding、Revoke 旧设备，从干净镜像重新部署，按 7.5 节重新审批和绑定。不要只删除 identity.json 或混用旧身份、Control/Gateway 凭据。Daemon 与 Helper 必须配套更新；未运行过的 Client 可直接使用本文的新配置。

### 8.5 Operational event journal日志库

**位置与配置。** 业务库与日志库是两个独立 SQLite 文件。`[storage].journal` 可省略，默认为业务库同目录的 `natsume-journal.db`；按本文业务库路径即 `/var/lib/natsume-server/natsume-journal.db`。自定义值必须是绝对路径，不能与业务库为同一路径／同一文件，符号链接和硬链接别名也会检查。旧配置不写 `[journal]` 或省略其中单项时使用默认值；这些值均必须大于零，修改后重启生效。

| 配置项 | 默认值 | 含义 |
| --- | --- | --- |
| `[journal].max_age_days` | `30` | 按事件发生时间保留的天数 |
| `[journal].max_rows` | `2000000` | 保留行数上限 |
| `[journal].max_bytes` | `536870912`（512 MiB） | 日志 DB＋WAL＋SHM 的合计预算，单位字节 |
| `[journal].min_free_bytes` | `1073741824`（1 GiB） | 日志所在文件系统可用空间下限，单位字节 |
| `[journal].flush_timeout_seconds` | `10` | 关闭时排空 writer 的最长等待秒数 |

`packaging/server/config.example.toml` 包含以上配置。日志库打开时自动创建／迁移文件，不创建缺失的父目录。日志是可裁剪的诊断历史，不参与业务事务或恢复；两个文件共用文件系统时，仍会争用磁盘空间，预算不是硬配额。

**权限与自定义路径。** Deb 通过 sysusers 创建 `natsume-server` 用户，通过 tmpfiles 创建 `/var/lib/natsume-server`（owner/group 均为 `natsume-server`，`0750`）。实际 unit 位于 `/usr/lib/systemd/system/natsume-server.service`，以该用户运行，未使用 `StateDirectory`；`ProtectSystem=strict`、`ProtectHome=yes`，`ReadWritePaths` 仅声明 `/var/lib/natsume-server` 和 `/var/log/natsume-server`。默认日志文件与 WAL／SHM 均位于允许写入的目录，文件系统权限也须允许服务用户创建、更新和删除它们。

自定义路径的父目录须预先创建，授予服务用户写入和遍历权限，并位于 unit 可写范围内；不能只改 TOML 后放到只读目录或被 `ProtectHome` 隔离的 home。若使用 `/srv/natsume-journal/natsume-journal.db`，先准备目录并添加 drop-in：

~~~bash
sudo install -d -o natsume-server -g natsume-server -m 0750 /srv/natsume-journal
sudo systemctl edit natsume-server.service
~~~

在编辑器中写入：

~~~ini
[Service]
ReadWritePaths=/srv/natsume-journal
~~~

这是追加允许路径，保留包内原有路径。将 `/etc/natsume-server/config.toml` 的 `[storage].journal` 改为上述绝对文件路径，再执行 `sudo systemctl daemon-reload` 和 `sudo systemctl restart natsume-server.service`。路径切换不自动迁移旧历史；需要保留时，按下文备份／恢复转移。可用以下命令核对配置与父目录权限，最终仍以服务内实际打开结果为准：

~~~bash
systemctl cat natsume-server.service
sudo -u natsume-server test -w /srv/natsume-journal
sudo -u natsume-server test -x /srv/natsume-journal
~~~

**预算、停写与恢复。** writer 启动时及每 5 分钟校准行数估计并清理过龄事件；按实际插入／删除数更新估计，仅超过 max_rows 时裁剪，不在每批写后执行 COUNT；每次短事务最多删除 5000 行。正常磁盘采样最多每秒一次，合计用量超过预算 80% 时预裁剪并尝试 PASSIVE checkpoint；达到预算或文件系统可用空间低于下限时进入 paused，普通事件停止插入、按类别计丢弃。暂停期间每秒删除最旧批次、尝试 TRUNCATE checkpoint 和 incremental vacuum；用量低于预算 60% 且可用空间至少达到下限才自动恢复。不能仅清理其他文件至下限就假定已恢复，还须满足日志用量条件；不补写停写期间丢失的事件。

日志库尚未打开或打开失败时为 degraded，Server 继续运行，事件计丢弃，每 60 秒重试打开。修复目录权限／挂载或释放空间后检查状态；不要通过修改业务库解决日志故障。暂停标记保留原发生时间，恢复时尽力写入 JournalPaused／JournalResumed；日志本身不可写或关闭超时时，健康标记也可能缺失。正常关闭先等待业务生产者和设备 lease，再 flush 日志；超时计丢失并写 tracing，`ServerStopping.lost_on_flush` 仅尽力保存。

观察入口：

- Panel 的 **Events** 和设备 **Timeline** 提示 degraded、paused 和本次 Server 启动以来的丢弃数；Events 还显示最早保留时间。读取失败有独立错误提示，不代表没有事件。
- admin 的 `GET /api/v2/journal/status` 返回队列深度估计、按类别的 dropped／write_failed／queued、pause_reason、last_error、shutdown_timeouts，以及 DB／WAL／SHM 大小和文件系统可用字节；测量失败的大小为空。计数随进程重启重新开始。
- `journalctl` 查看日志库 unavailable、batch write failed、checkpoint incomplete、maintenance failed 和 flush timed out 等 tracing 诊断；停写／恢复状态以 status 和 Panel 为准。

用第 5.3 节的 admin 会话变量查询（会话失效先重新登录）：

~~~bash
curl --fail-with-body --silent --show-error --http1.1 \
  --cacert "$NATSUME_CONTROL_CA" --cookie "$NATSUME_API_DIR/cookies" \
  "$NATSUME_SERVER_ORIGIN/api/v2/journal/status"
sudo journalctl -u natsume-server.service --since '1 hour ago' --no-pager
~~~

viewer 仅可读取 device 类事件、设备时间线和概要，不能访问 status 或导出。所有日志 GET 响应 no-store，不再产生请求审计；事件时间按 Server 启动时捕获的 UTC offset 显示。Device 表的 Drops (24h) 是保留历史中的断开次数（至少 3 次标记 flapping），与队列丢弃数不同。日志不包含密码、token、key、CSR／证书正文、完整 HWID 或尝试登录名；历史仍含设备／操作者 ID、IP 和座位等信息，应限制备份访问。

**独立在线备份。** 无需停止 Server，使用 SQLite `.backup` 获取一致的日志库副本；不要在运行中仅复制 `.db`，也不要分别复制运行中的 WAL／SHM。先确认日志库确实存在，避免 sqlite3 把错误路径建成空库。下例使用默认日志路径，自定义时同步修改变量：

~~~bash
NATSUME_JOURNAL_DB=/var/lib/natsume-server/natsume-journal.db
NATSUME_JOURNAL_BACKUP_DIR="/var/backups/natsume/journal-$(date -u +%Y%m%dT%H%M%SZ)"
sudo -u natsume-server test -f "$NATSUME_JOURNAL_DB"
# 上一条成功后继续。
sudo install -d -o root -g natsume-server -m 0710 /var/backups/natsume
sudo install -d -o natsume-server -g natsume-server -m 0700 "$NATSUME_JOURNAL_BACKUP_DIR"
sudo -u natsume-server sqlite3 "$NATSUME_JOURNAL_DB" \
  ".backup '$NATSUME_JOURNAL_BACKUP_DIR/natsume-journal.db'"
sudo -u natsume-server sqlite3 "$NATSUME_JOURNAL_BACKUP_DIR/natsume-journal.db" \
  'PRAGMA integrity_check;'
sudo chown root:root "$NATSUME_JOURNAL_BACKUP_DIR" "$NATSUME_JOURNAL_BACKUP_DIR/natsume-journal.db"
sudo chmod 0600 "$NATSUME_JOURNAL_BACKUP_DIR/natsume-journal.db"
~~~

确认 `.backup` 成功且完整性为 ok，再按现场备份制度复制到独立存储，并保存匹配 Server 版本。两库的独立备份没有跨库时间点一致性保证；日志中的 actor ID 可能无法在恢复后的业务库解析显示名，仍可显示 ID。admin 的 JSONL 导出适合诊断分享，固定 high-water 并在流尾报告期间裁剪，不是可还原 SQLite 文件的备份。

**恢复或放弃历史。** 只恢复日志库时无需回滚业务库，不运行 bootstrap。维护窗口先停止唯一 Server 并确认没有手动 serve；保存现有日志 DB 及配套 WAL／SHM 到故障副本目录，禁止新旧配套文件混用。以下路径中的备份日期须替换为实际备份集：

~~~bash
sudo systemctl stop natsume-server.service
systemctl is-active natsume-server.service
~~~

预期 inactive；确认后继续：

~~~bash
NATSUME_JOURNAL_DB=/var/lib/natsume-server/natsume-journal.db
NATSUME_JOURNAL_BACKUP=/var/backups/natsume/journal-20261028T000000Z/natsume-journal.db
NATSUME_JOURNAL_FAILED_DIR="/var/backups/natsume/journal-failed-$(date -u +%Y%m%dT%H%M%SZ)"
sudo test -f "$NATSUME_JOURNAL_BACKUP"
# 确认备份文件存在后继续。
sudo sqlite3 "$NATSUME_JOURNAL_BACKUP" 'PRAGMA integrity_check;'
# 确认 ok 后继续；保留故障现场，移走全部旧配套文件。
sudo install -d -o root -g root -m 0700 "$NATSUME_JOURNAL_FAILED_DIR"
for suffix in '' '-wal' '-shm'; do
  if sudo test -e "${NATSUME_JOURNAL_DB}${suffix}"; then
    sudo mv -- "${NATSUME_JOURNAL_DB}${suffix}" "$NATSUME_JOURNAL_FAILED_DIR/"
  fi
done
sudo install -o natsume-server -g natsume-server -m 0600 \
  "$NATSUME_JOURNAL_BACKUP" "$NATSUME_JOURNAL_DB"
sudo systemctl start natsume-server.service
~~~

恢复后检查 status、Panel 历史边界及业务设备重连。若不保留日志历史，停服并移走旧 DB／WAL／SHM 后不执行 install 恢复，直接启动即可在可写父目录重建日志库。丢失／删除仅使诊断历史不可恢复，业务 authority、vault 和业务恢复不受影响；运行时不要删除或替换打开的日志文件，重启后也不会重建过去的事件或计数。已裁剪或未成功持久化的事件无法从业务库补全。离线 `reset-operator-password` 不打开／写入日志库。

## 9. 故障定位表

| 现象 | 先检查 | 处理方向 |
| --- | --- | --- |
| unit skipped | 三个必需输入、systemctl status | 部署完整配置和 CA 后启动 |
| Server TLS identity invalid | DER/PKCS#8、公钥匹配、keys 0700/files 0600 | 按附录 A.5 检查，不把 PEM 改后缀冒充 DER |
| Origin CA mismatch | public PEM 解码与 origin-ca.der | 必须同一证书且私钥匹配，不能只比较 CN |
| Nginx 检查证书报 Permission denied | 是否以普通用户执行 nginx -t，证书路径及目录权限 | 按 2.7 节设置权限，用 sudo nginx -t 检查成功后再重载 |
| Client TLS 失败 | 时钟、Control CA、IP SAN、端口 | 从 Client 用不带 -k 的 curl/openssl 检查 |
| 配置读取失败（NotFound / PermissionDenied） | 报错中的配置路径、服务用户读取权限 | 确认 /etc/natsume-server/config.toml 存在且 natsume-server 可读 |
| missing required field | 报错指出的 section 或字段 | 对照附录 A.7 补齐完整配置 |
| TOML syntax or field type | 报错的行列、引号和值类型 | 修正 TOML 后重试 |
| runtime.domjudge_origin 校验失败 | 实际上游是否提供 HTTPS，地址是否只包含 origin | HTTP 地址不受支持；使用可用的 HTTPS 上游，不带末尾斜杠或路径 |
| 无 Enrollment review | 窗口、Devices 列表、控制连接与注册握手 | Open 会自动批准，先查 Devices；Closed 应有待审请求，检查 Client/Server 日志 |
| 无法绑定 | XLSX 是否 commit、工位是否存在/被占 | 处理数据与 Binding，不删除设备身份 |
| Firefox 不信任 Gateway | about:policies、Install路径、CA/hostname/期限 | 修正策略并完全重启 Firefox |
| Gateway 连接失败/503 | loopback解析、listener、Gateway/Binding/Runtime | 先完成注册配置，BLOCKED 不能靠关闭 TLS 验证解决 |
| Gateway 502/上游 TLS 失败 | DNS、系统CA、上游端口/证书 | Client 用系统信任测试，额外 CA 按 7.3 节分发 |
| 页面可访问但登录错误 | 工位账号、密码、登录头支持、Cookie/重定向 | 在 teams Firefox 实测 /login，不打印 Caddy 凭据 |
| 持续橙色桌面/无 Agent | waiting Kiosk、GDM/Wayland、Helper报告 | 独立管理员维护，不旁路再启动一个 Agent |
| contest 不呈现/Home recovery_required | 模板 mount、Home、Binding、会话报告 | 保存现场数据，按镜像恢复流程处理，不自动删 Home |
| 重装/克隆后身份冲突 | 硬件身份、镜像内旧状态 | 干净通用镜像、每台独立身份，按审批迁移 |
## 10. 实施依据

- [首次安装工具](../crates/natsume-server-setup/README.md)、[安装工具决策](adr/0001-server-setup-installer.md)。
- [Server 命令、配置和私有材料](../crates/natsume-server/README.md)、[Server 配置示例](../packaging/server/config.example.toml)、[Client 配置示例](../packaging/client/config.example.toml)。
- [Server 配置解析](../crates/natsume-server/src/config.rs)、[TLS 加载](../crates/natsume-server/src/tls.rs)、[Gateway CA 签发](../crates/natsume-server/src/component/gateway/issuer.rs)。
- [Runtime Config 读取](../crates/natsume-server/src/component/runtime.rs)、[数据库 migration 序列](../crates/natsume-server/diesel/business/migrations/)、[注册窗口 API](../crates/natsume-server/src/http/handler/provisioning.rs)。
- [日志 writer](../crates/natsume-server/src/journal/writer.rs)、[磁盘预算](../crates/natsume-server/src/journal/budget.rs)、[日志库 migration](../crates/natsume-server/diesel/journal/migrations/)、[Journal 只读 API](../crates/natsume-server/src/http/handler/journal.rs)。
- [Server systemd unit](../packaging/server/rootfs/usr/lib/systemd/system/natsume-server.service)、[状态目录 tmpfiles](../packaging/server/rootfs/usr/lib/tmpfiles.d/natsume-server.conf)。
- [构建与发布](../packaging/README.md)、[Client manifest](../packaging/client/nfpm.yaml)、[Client 运行环境要求](client-environment.zh-CN.md)。

架构目标由 [architecture.md](architecture.md) 定义。

## 附录 A：手工部署流程

本附录保留逐项手工流程。它与第 4 节 setup 路线二选一；包安装仍按第 3 节下载校验、第 4.1 节安装。先执行 A.1–A.5，再执行 A.6–A.9，最后回到第 5 节。

### A.1 建立受控工作目录

**执行位置：Server，使用管理员账号。**先安装 OpenSSL：

~~~bash
sudo apt-get update
sudo apt-get install --yes openssl
~~~

选择一个新目录，避免覆盖既有 CA：

~~~bash
umask 077
NATSUME_PKI_DIR="$HOME/natsume-pki-2026"
mkdir -m 0700 "$NATSUME_PKI_DIR"
cd "$NATSUME_PKI_DIR"
mkdir -m 0700 private public server
openssl version
~~~

本节后续命令都在该目录执行。OpenSSL 会交互询问私钥口令；分别设置强口令，通过密码管理器保存，不放进命令历史。

### A.2 创建 Control CA

~~~bash
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -aes-256-cbc -out private/control-ca-key.pem

openssl req -new -x509 -sha256 -days 3650 \
  -key private/control-ca-key.pem \
  -subj '/O=Contest Operations/CN=Natsume Control Root CA 2026' \
  -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -addext 'subjectKeyIdentifier=hash' \
  -out public/control-ca.crt
~~~

3650 是示例 CA 有效天数，名称和期限按部署策略调整。Control CA 私钥留在本机 PKI 的 private/ 目录，不复制到服务私有目录。

### A.3 创建 Local Origin CA，导出在线材料

~~~bash
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -aes-256-cbc -out private/local-origin-ca-key.pem

openssl req -new -x509 -sha256 -days 3650 \
  -key private/local-origin-ca-key.pem \
  -subj '/O=Contest Operations/CN=Natsume Local Origin Root CA 2026' \
  -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -addext 'subjectKeyIdentifier=hash' \
  -out public/local-origin-ca.crt

openssl x509 -in public/local-origin-ca.crt -outform DER \
  -out server/origin-ca.der
openssl pkcs8 -topk8 -nocrypt -outform DER \
  -in private/local-origin-ca-key.pem -out server/origin-ca-key.pk8
~~~

origin-ca-key.pk8 使用 Server 要求的**未加密 PKCS#8 DER** 编码，只能作为在线签发密钥部署到 Server 受限目录。参数见 [OpenSSL pkcs8](https://docs.openssl.org/3.0/man1/openssl-pkcs8/)。

Local Origin CA 有效期必须覆盖 Gateway not after。

### A.4 签发含 Server IP SAN 的 TLS leaf

修改变量为 Client 实际连接的 IP。必须包含 IP SAN，只有 CN 或 DNS SAN 不足以校验 IP 端点。

~~~bash
NATSUME_SERVER_IP='192.0.2.10'

openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -aes-256-cbc -out private/server-tls-key.pem
openssl req -new -sha256 -key private/server-tls-key.pem \
  -subj '/O=Contest Operations/CN=Natsume Server' \
  -out server/server-tls.csr

cat > server/server-tls.ext <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=IP:$NATSUME_SERVER_IP
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF

openssl x509 -req -sha256 -days 365 \
  -in server/server-tls.csr \
  -CA public/control-ca.crt -CAkey private/control-ca-key.pem \
  -CAcreateserial -CAserial private/control-ca.srl \
  -extfile server/server-tls.ext -out server/server-tls-leaf.pem

openssl x509 -in server/server-tls-leaf.pem -outform DER \
  -out server/server-tls-leaf.der
openssl pkcs8 -topk8 -nocrypt -outform DER \
  -in private/server-tls-key.pem -out server/server-tls-key.pk8
~~~

如需多个 IP，签发前将 SAN 改为 IP:地址1,IP:地址2。IPv6 SAN 使用原始地址，不带方括号。本文后续命令按 IPv4；IPv6 URL 需要 https://[地址]:8443，监听如 [::]:8443。

365 天从签发时刻起算，必须覆盖部署和比赛期间且不超出 Control CA 有效期。扩展参数依据 [OpenSSL req](https://docs.openssl.org/3.0/man1/openssl-req/)、[x509](https://docs.openssl.org/3.0/man1/openssl-x509/) 和 [X.509 扩展格式](https://docs.openssl.org/3.0/man5/x509v3_config/)。

### A.5 验证证书、编码和公钥匹配

~~~bash
openssl verify -CAfile public/control-ca.crt public/control-ca.crt
openssl verify -CAfile public/local-origin-ca.crt public/local-origin-ca.crt
openssl verify -CAfile public/control-ca.crt -purpose sslserver \
  -verify_ip "$NATSUME_SERVER_IP" server/server-tls-leaf.pem

openssl x509 -in public/control-ca.crt -noout -subject -dates -fingerprint -sha256
openssl x509 -in public/local-origin-ca.crt -noout -subject -dates -fingerprint -sha256
openssl x509 -in server/server-tls-leaf.pem -noout -dates -ext subjectAltName

openssl x509 -in public/local-origin-ca.crt -outform DER -out server/origin-ca-check.der
cmp server/origin-ca.der server/origin-ca-check.der

openssl x509 -in server/server-tls-leaf.pem -pubkey -noout > server/tls-cert-public.pem
openssl pkey -inform DER -in server/server-tls-key.pk8 -pubout > server/tls-key-public.pem
cmp server/tls-cert-public.pem server/tls-key-public.pem

openssl x509 -in public/local-origin-ca.crt -pubkey -noout > server/origin-cert-public.pem
openssl pkey -inform DER -in server/origin-ca-key.pk8 -pubout > server/origin-key-public.pem
cmp server/origin-cert-public.pem server/origin-key-public.pem

openssl pkcs8 -inform DER -nocrypt -in server/server-tls-key.pk8 -out /dev/null
openssl pkcs8 -inform DER -nocrypt -in server/origin-ca-key.pk8 -out /dev/null
~~~

预期 verify 均为 OK，cmp 无输出且退出码为 0，两份 PKCS#8 可解析。将公共 CA 指纹和所有期限归档，通过独立可信渠道供接收方核对。

**不要预先生成 Client 身份文件、Control key、Gateway key/leaf 或设备 token**；它们由真实机器首次启动和注册生成。

### A.6 整理并复制 PKI 输入

先按第 3.1 节建立部署目录。

以下文件已在本机 $HOME/natsume-pki-2026 中生成，本节将其复制到部署目录。加密私钥原件和签发记录继续保留在 PKI 目录：

| PKI 路径 | 部署目录下路径 | 最终接收方 |
| --- | --- | --- |
| public/control-ca.crt | public/control-ca.crt | Server、Client、管理员浏览器 |
| public/local-origin-ca.crt | public/local-origin-ca.crt | Server、Client |
| server/origin-ca.der | server/origin-ca.der | Server |
| server/origin-ca-key.pk8 | server/origin-ca-key.pk8 | **仅 Server，秘密** |
| server/server-tls-leaf.der | server/server-tls-leaf.der | Server |
| server/server-tls-key.pk8 | server/server-tls-key.pk8 | **仅 Server，秘密** |

只复制表中列出的文件，不把 PKI 的 private/ 整目录交给服务。Client 部署方只接收其配置和公共证书，不接收 server/ 目录。

**执行位置：Server，继续使用同一管理员账号。**从本机 PKI 目录复制已验证的证书和在线私钥到第 3.1 节创建的部署目录：

~~~bash
umask 077
NATSUME_PKI_DIR="$HOME/natsume-pki-2026"
NATSUME_DEPLOY_DIR="$HOME/natsume-deploy"
cp "$NATSUME_PKI_DIR/public/control-ca.crt" \
  "$NATSUME_PKI_DIR/public/local-origin-ca.crt" \
  "$NATSUME_DEPLOY_DIR/public/"
cp "$NATSUME_PKI_DIR/server/origin-ca.der" \
  "$NATSUME_PKI_DIR/server/origin-ca-key.pk8" \
  "$NATSUME_PKI_DIR/server/server-tls-leaf.der" \
  "$NATSUME_PKI_DIR/server/server-tls-key.pk8" \
  "$NATSUME_DEPLOY_DIR/server/"
chmod 0600 "$NATSUME_DEPLOY_DIR/server/origin-ca.der" \
  "$NATSUME_DEPLOY_DIR/server/origin-ca-key.pk8" \
  "$NATSUME_DEPLOY_DIR/server/server-tls-leaf.der" \
  "$NATSUME_DEPLOY_DIR/server/server-tls-key.pk8"
~~~

Server Deb 直接使用第 3.2 节已下载并校验的本地文件。先按第 4.1 节安装软件包，再按附录 A.7 复制到最终服务路径。

### A.7 安装证书、密钥和完整配置

~~~bash
sudo install -d -o root -g root -m 0755 /etc/natsume-server /etc/natsume /etc/natsume/trust
sudo install -o root -g root -m 0644 \
  "$NATSUME_DEPLOY_DIR/public/control-ca.crt" /etc/natsume/trust/control-ca.crt
sudo install -o root -g root -m 0644 \
  "$NATSUME_DEPLOY_DIR/public/local-origin-ca.crt" /etc/natsume/trust/local-origin-ca.crt

sudo install -d -o natsume-server -g natsume-server -m 0700 /var/lib/natsume-server/keys
sudo install -o natsume-server -g natsume-server -m 0600 \
  "$NATSUME_DEPLOY_DIR/server/origin-ca.der" \
  "$NATSUME_DEPLOY_DIR/server/origin-ca-key.pk8" \
  "$NATSUME_DEPLOY_DIR/server/server-tls-leaf.der" \
  "$NATSUME_DEPLOY_DIR/server/server-tls-key.pk8" \
  /var/lib/natsume-server/keys/
~~~

创建完整配置，按清单替换 hostname、端口、日期和 DOMjudge 上游 origin：

~~~bash
cat > "$NATSUME_DEPLOY_DIR/server/config.toml" <<'EOF'
[listen]
https = "0.0.0.0:8443"

[log]
level = "info"

[storage]
database = "/var/lib/natsume-server/natsume.db"
journal = "/var/lib/natsume-server/natsume-journal.db"
root_key = "/var/lib/natsume-server/keys/server-root.key"
organization_logos = "/var/lib/natsume-server/organization-logos"
print_documents = "/var/lib/natsume-server/print-documents"

[journal]
max_age_days = 30
max_rows = 2000000
max_bytes = 536870912
min_free_bytes = 1073741824
flush_timeout_seconds = 10

[tls]
certificate = "/var/lib/natsume-server/keys/server-tls-leaf.der"
private_key = "/var/lib/natsume-server/keys/server-tls-key.pk8"

[site]
gateway_hostname = "domjudge"
gateway_not_after = "2026-12-08T10:00:00Z"
contest_end = "2026-12-06T10:00:00Z"

[trust]
control_root = "/etc/natsume/trust/control-ca.crt"
local_origin_root = "/etc/natsume/trust/local-origin-ca.crt"

[runtime]
domjudge_origin = "https://judge.contest.example"
EOF

sudo install -o root -g root -m 0644 \
  "$NATSUME_DEPLOY_DIR/server/config.toml" /etc/natsume-server/config.toml
~~~

0.0.0.0 是 IPv4 监听地址，Client 要写真正 Server IP。domjudge_origin 必须为实际 DOMjudge 的 canonical HTTPS origin，不含路径、末尾斜杠、用户名密码、查询参数或 fragment；不要填 Client 的本机 Gateway 地址。本文使用默认绝对路径；自定义路径还涉及 systemd 沙箱范围，不能只修改 TOML。

检查语法、期限与权限：

~~~bash
python3 - <<'PY'
import datetime as dt
import pathlib
import tomllib

c = tomllib.loads(pathlib.Path('/etc/natsume-server/config.toml').read_text())
end = dt.datetime.fromisoformat(c['site']['contest_end'])
expiry = dt.datetime.fromisoformat(c['site']['gateway_not_after'])
assert end.utcoffset() is not None and expiry.utcoffset() is not None
assert expiry >= end + dt.timedelta(days=1)
assert expiry > dt.datetime.now(dt.timezone.utc)
print('TOML syntax and Gateway validity window: OK')
PY

sudo stat -c '%U:%G %a %n' /var/lib/natsume-server/keys \
  /var/lib/natsume-server/keys/origin-ca.der \
  /var/lib/natsume-server/keys/origin-ca-key.pk8 \
  /var/lib/natsume-server/keys/server-tls-leaf.der \
  /var/lib/natsume-server/keys/server-tls-key.pk8
sudo -u natsume-server test -r /etc/natsume-server/config.toml
sudo -u natsume-server test -r /var/lib/natsume-server/keys/origin-ca-key.pk8
~~~

keys 目录应为 natsume-server:natsume-server 700，文件 600。进程启动时进一步验证完整配置、编码、证书/私钥匹配和 Origin CA 一致性。

### A.8 初始化数据库

首次部署，在交互 TTY 运行：

~~~bash
sudo -u natsume-server -- /usr/bin/natsume-server bootstrap
~~~

按提示输入管理员登录名及两次密码。提示会先说明账号用途和密码规则；登录名为空、密码不符合策略或两次输入不一致时显示原因并重新询问，Ctrl-C 终止。必须使用服务用户，不直接以 root 运行，不放入 Deb 安装脚本或自动化任务。bootstrap 会创建/迁移完整数据库 schema、生成缺失的 server-root.key，并在同一事务中创建首个管理员及写入配置中的 DOMjudge 上游，成功后退出。

管理员和 Runtime Config 要么同时写入，要么同时回滚。若首次 bootstrap 中途失败，已完成的建表迁移和已生成的 vault key 可能保留；修正错误后可以重试，不要删除或重新生成已有 key。比赛工位、队伍账号和设备记录由后续导入、注册创建，初始化时不填充虚构数据。

~~~bash
sudo stat -c '%U:%G %a %n' \
  /var/lib/natsume-server/natsume.db \
  /var/lib/natsume-server/keys/server-root.key
~~~

重复 bootstrap 会非零退出，不创建第二个管理员，也不覆盖已有 Runtime Config。后续 systemd 使用 serve 启动，不会自动创建 DB/key/account；未完成管理员初始化时启动失败。上游变更由部署方修改 config.toml 后重启服务，启动时会同步到 Runtime Config。

可用只读查询核对初始化结果：

~~~bash
sudo -u natsume-server sqlite3 -readonly /var/lib/natsume-server/natsume.db \
  'SELECT singleton, domjudge_origin FROM runtime_config; PRAGMA integrity_check;'
~~~

预期只有一行 singleton=1 的配置上游，完整性检查为 ok。

### A.9 启动并验证 TLS

~~~bash
sudo systemctl daemon-reload
sudo systemctl enable --now natsume-server.service
sudo systemctl status natsume-server.service --no-pager
sudo journalctl -u natsume-server.service -b -n 100 --no-pager
sudo ss -lntp 'sport = :8443'
~~~

预期 active (running) 且监听端口。skipped 先检查三个必需输入；反复重启查日志，不反复 bootstrap。

从 Server 验证实际 IP：

~~~bash
NATSUME_SERVER_IP='192.0.2.10'
curl --fail --show-error --http1.1 --tlsv1.3 \
  --cacert /etc/natsume/trust/control-ca.crt \
  "https://$NATSUME_SERVER_IP:8443/api/v2/health"

openssl s_client -connect "$NATSUME_SERVER_IP:8443" \
  -CAfile /etc/natsume/trust/control-ca.crt \
  -verify_ip "$NATSUME_SERVER_IP" -verify_return_error \
  -tls1_3 -alpn http/1.1 </dev/null
~~~

应 HTTP 成功、证书验证成功、ALPN 为 http/1.1；禁止使用 curl -k。


### A.10 离线 CA 签发 setup 生成的 CSR

**执行位置：离线 CA 主机。**仅接收 pending/server-tls.csr、control-ca.crt、request.toml，置于新建受限工作目录；不要传输 Server 私钥。准备与该证书匹配的加密 Control CA 私钥（以下示例 /secure/control-ca-key.pem）。先经可信渠道核对 CA 指纹、request.toml 中所有参数和 CSR 公钥指纹；不接受未审核的请求。

~~~bash
umask 077
cd /secure/natsume-signing-request
openssl req -in server-tls.csr -noout -verify -text
openssl x509 -in control-ca.crt -noout -subject -dates -fingerprint -sha256
openssl x509 -in control-ca.crt -pubkey -noout > ca-public.pem
openssl pkey -in /secure/control-ca-key.pem -pubout > key-public.pem
cmp ca-public.pem key-public.pem
~~~

以下从审核后的 request.toml 构造扩展，显式保留**全部 IP SAN**、serverAuth 与 digitalSignature。签发天数取 365 天与 CA 剩余整天（再留一天余量）的较小值，且必须覆盖 gateway_not_after；不足则停止，由 CA 管理员安排合适期限，不能缩短请求的覆盖要求：

~~~bash
python3 - <<'PYCSR'
import datetime as dt
import hashlib
import ipaddress
import pathlib
import subprocess
import tomllib

record = tomllib.loads(pathlib.Path('request.toml').read_text())
ca_der = subprocess.check_output(['openssl', 'x509', '-in', 'control-ca.crt', '-outform', 'DER'])
assert hashlib.sha256(ca_der).hexdigest() == record['control_ca_sha256']
csr_der = subprocess.check_output(['openssl', 'req', '-in', 'server-tls.csr', '-pubkey', '-noout'])
csr_spki = subprocess.check_output(['openssl', 'pkey', '-pubin', '-outform', 'DER'], input=csr_der)
assert hashlib.sha256(csr_spki).hexdigest() == record['csr_public_key_sha256']
now = dt.datetime.now(dt.timezone.utc)
ca_end_text = subprocess.check_output(['openssl', 'x509', '-in', 'control-ca.crt', '-noout', '-enddate'], text=True)
ca_end = dt.datetime.strptime(ca_end_text.strip().split('=', 1)[1], '%b %d %H:%M:%S %Y %Z').replace(tzinfo=dt.timezone.utc)
days = min(365, int((ca_end - now).total_seconds() // 86400) - 1)
expiry = dt.datetime.fromisoformat(record['gateway_not_after'])
assert days > 0 and now + dt.timedelta(days=days) >= expiry, 'CA/leaf window cannot cover Gateway expiry'
sans = ','.join('IP:' + str(ipaddress.ip_address(ip)) for ip in record['server_ips'])
pathlib.Path('server-tls.ext').write_text(
    'basicConstraints=critical,CA:FALSE\n'
    'keyUsage=critical,digitalSignature\n'
    'extendedKeyUsage=serverAuth\n'
    f'subjectAltName={sans}\n'
    'subjectKeyIdentifier=hash\n'
    'authorityKeyIdentifier=keyid,issuer\n')
pathlib.Path('leaf-days').write_text(str(days))
PYCSR

NATSUME_LEAF_DAYS="$(cat leaf-days)"
openssl x509 -req -sha256 -days "$NATSUME_LEAF_DAYS" \
  -in server-tls.csr -CA control-ca.crt -CAkey /secure/control-ca-key.pem \
  -CAcreateserial -CAserial control-ca.srl \
  -extfile server-tls.ext -out server-tls-leaf.pem
openssl x509 -in server-tls-leaf.pem -noout -dates -ext subjectAltName
openssl verify -partial_chain -CAfile control-ca.crt -purpose sslserver server-tls-leaf.pem
~~~

Control CA **直接签发**此 leaf，不增设中间 CA；私钥口令由 OpenSSL 交互读取。核对生成 leaf 的 notAfter 覆盖 request.toml 的 gateway_not_after 且不超过 Control CA，到期边界以实际证书为准；逐个 IP 用 openssl verify -partial_chain -CAfile control-ca.crt -purpose sslserver -verify_ip 实际IP server-tls-leaf.pem 验证。只返回 server-tls-leaf.pem，回到第 4.3 节让 setup 再次执行完整校验。

## 历史迁移参考（仅适用于 v2.3.2）

以下记录仅用于迁移到 v2.3.2；升级到当前版本须另行核对数据库、配置和镜像兼容性。历史安装包应从 [v2.3.2 Release](https://github.com/4o3F/Natsume/releases/tag/v2.3.2) 单独下载并核对该版本的 SHA256SUMS，不使用第 3.2 节选定的当前版本包。旧镜像的 Wayland、GDM、dconf 和 Home 迁移由镜像项目完成，不再依赖随包镜像适配材料。

<a id="upgrade-from-v2-0"></a>
### 从 v2.0.4 配套升级到 v2.3.2（Breaking Change）

从 v2.0.4 升级涉及控制协议的不兼容变化。Server、Device Daemon、Helper、Session Agent 与 Web 必须配套；控制连接使用 `natsume.control.v3`，本地 Agent 注册使用协议 3。旧 Client 不能继续连接新 Server。新版绑定目标必须包含队伍／学校资料，因此须先补全名单，再让设备重连。

1. 在维护窗口完成已开始的 Home reset／会话维护，保存比赛数据，暂停新操作。按[运行环境的维护要求](client-environment.zh-CN.md#维护要求)退出受管会话并停止 Client 控制连接；确认 Server 已显示设备离线。
2. 按第 8.2 节保存一致的数据库、密钥与配置冷备份，另行保存 Logo 目录、完整名单和匹配旧包。备份步骤结束后再次停止 Server，保持停止状态进行软件包升级。
3. 安装从该历史 Release 校验过的 `natsume-server_2.3.2_amd64.deb`。在已有 `/etc/natsume-server/config.toml` 的 `[storage]` 中补上必填绝对路径 `organization_logos`，参考附录 A.7。保留原数据库和全部密钥，**不要重新 bootstrap，也不要删除数据库或设备身份**。
4. 显式启动 Server；`serve` 在启动时执行数据库迁移，保留已有 Device、Enrollment、Account、Vault、Binding 和 Session 目标。迁移会移除旧格式的未提交导入候选，须重新上传完整 XLSX。
5. 保持 Client 停止，通过新版 Panel 按第 5.2 节上传完整名单并提交。账号与座位须与现有数据一致；不准备改密时填写原密码。核对学校、队伍资料和已有绑定，准备 Logo。没有 Logo 允许使用默认图；缺少队伍资料的已绑定账号不能生成有效控制目标。
6. 安装配套 `natsume-client_2.3.2_amd64.deb`，由镜像项目检查并应用匹配的配置差异，包括下文 Wayland 迁移所需的会话、GDM 和 dconf 配置。包会提供 Noto CJK 字体依赖和展示目录；保留原 Client 身份、Control/Gateway 凭据及 Home 维护状态。确认 `/var/lib/natsume-display` 为 natsume 所有、0755，waiting 只能读取图片。
7. 先恢复一台测试设备，再逐批恢复。核对设备 Online、原绑定／座位、waiting 队伍资料、Logo 和五项资源状态。检查切换、断线缓存、解绑后不恢复旧队伍，并按[运行环境要求](client-environment.zh-CN.md)在实际目标机器完成验证。

需要回退时，恢复同一维护点的旧版本 Server/Client、数据库与密钥／配置备份；不能把新数据库与旧二进制混用。更早的站点 UUID Client 仍按第 8.4 节处理，不套用本节的身份保留流程。

<a id="upgrade-v2-2-0"></a>
### 从 v2.1.0 升级到 v2.3.2（HTTP API 与 Client 图形栈 Breaking Change）

本次控制连接仍使用 `natsume.control.v3`，本地 Agent 协议仍为 3；现有 v2.1.0 Client 可继续连接。 Server 包内的 Web 与 Server 一起升级。自定义运维脚本必须迁移旧 Session/Home 单台写接口。Client 升级到 v2.3.2 时，还须完成下文的 Wayland 迁移。

1. 暂停新目标操作，按第 8.2 节保存一致的数据库、密钥与配置冷备份，保留原安装包用于回退。
2. 安装从该历史 Release 校验过的 `natsume-server_2.3.2_amd64.deb`，随后显式重启 `natsume-server.service`。 启动时自动应用第三条 migration，增加提交去重记录表；既有设备、绑定、账号和目标保留。**不要重新 bootstrap。**
3. 完全刷新管理员 Web 页面。Targets 默认展示 Enabled/Disabled，可切换生命周期筛选；单台与批量操作统一提交，操作对象仅限 Enabled，包含离线设备。
4. 外部脚本改用 `POST /api/v2/target-submissions`，请求带唯一 `operation_id`、设备范围和一种动作。 旧 `PUT /devices/{device_id}/session-control`、`POST /devices/{device_id}/session-control/actions/terminate` 与 `POST /devices/{device_id}/home/actions/reset` 写入口已删除，原读取接口保留。
5. 单台验证 waiting/contest 切换及提交结果，再按现场需要使用全部设备操作。HTTP 成功表示 Server 已保存结果，终端完成仍看 Session/Home convergence。 网络结果不明时用原操作 ID 和原内容重试；Web 会保留待确认请求，刷新后可点击 **Retry original request**。 明确拒绝的设备使用 **Retry failed devices** 创建新操作，仅重试失败名单。

去重记录随当前比赛数据库保留，不随注销、设备禁用或撤销删除。数据库回退会回退这些记录； 回退时使用同一维护点的包与数据库，不能向回退后的系统重放升级后生成的待确认请求。

<a id="upgrade-v2-3-0"></a>
### 从 v2.2.x 升级到 v2.3.2（Client 图形栈 Breaking Change）

本版本没有新增数据库 migration，控制连接仍使用 `natsume.control.v3`，本地 Agent 协议仍为 3。 Server 升级保留设备、绑定、目标和提交去重记录；原有协议兼容的 Client 仍可连接。 Client v2.3.2 的 Agent、Helper 和会话入口要求原生 Wayland，必须与镜像配置配套更新。

1. 在维护窗口结束比赛操作，保存 teams 的工作文件，保留独立管理员 SSH/TTY 入口；按第 8.2 节完成一致冷备份。
2. 安装校验过的 `natsume-server_2.3.2_amd64.deb`，显式重启 Server，完全刷新管理员 Web。**不要重新 bootstrap 或删除数据库。**
3. 按[运行环境的维护要求](client-environment.zh-CN.md#维护要求)退出受管会话并停止 Client 控制连接，安装 `natsume-client_2.3.2_amd64.deb`。保留设备身份、Control/Gateway 密钥、Binding、Home 维护状态和展示缓存。
4. 由镜像项目完成与新 Client 匹配的桌面适配，满足[运行环境要求](client-environment.zh-CN.md)：启用原生 Wayland，waiting 使用 `gnome-kiosk-script-wayland`，teams 使用 `ubuntu-wayland`，确认官方会话入口存在。
5. 清理旧的强制 `WaylandEnable=false` / `PreferredDisplayServer=xorg` GDM drop-in、旧 Natsume Xorg ServerFlags 和 Kiosk X11 frame-scale 覆盖。合并新版 dconf 数据库及 locks，在目标系统执行 `dconf update`。保留完整官方 PAM 栈与 Home 门禁。
6. 正常重启 Client，确认 Helper、Daemon、Agent 及图形会话全部加载新版本，waiting/teams 均为 seat0 的原生 `Type=wayland` 会话，绑定和设备身份保持不变。
7. 核对实际 waiting 全屏、队伍资料、键盘输入、teams Home 图标和完整右键菜单，再验证普通切换、Home reset、断线重连及冷启动恢复当前目标。初始化和重建允许短暂 greeter/闪屏。

回退时使用匹配的旧 Client 与 X11 镜像配置及维护备份，不能只降级 Helper、Agent 或单个二进制。

<a id="upgrade-v2-3-1"></a> <a id="upgrade-v2-3-2"></a>
### 从 v2.3.0／v2.3.1 升级到 v2.3.2

本补丁将 Rustls 升级到 0.23.45，修复 [RUSTSEC-2026-0285](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc)，并包含 Gateway 本机解析与 Firefox 入口修复。不改变数据库 schema、Device Control 或本地 Agent 协议；已完成原生 Wayland 集成的 v2.3.0 镜像可以原地升级。更早的 Client 仍须先按上述 Wayland 迁移要求完成镜像配置迁移。

1. 在维护期保存正在使用的比赛文件。安装校验过的完整 `natsume-client_2.3.2_amd64.deb`，保留设备身份、凭据、绑定、Home 状态以及现有完整 `config.toml` 和 CA。
2. 执行 `sudo systemctl restart natsume-privileged-helper.service natsume-device-daemon.service`，使两个进程加载配套版本。Daemon 从配置解析 Gateway 域名并传给 Helper，Helper 不读取 Client 配置；首次同步不需要 Server 在线或重新 Enrollment。
3. 完全退出再打开 Firefox。按第 7.4 节检查实际域名仅解析到 loopback，Firefox 的主页及比赛书签均使用配置域名，其他地址、书签和 CA 策略保留。无需重新制作镜像或手工编辑 hosts。
4. 安装 `natsume-server_2.3.2_amd64.deb` 并显式重启 `natsume-server.service`，使 Server HTTPS/WSS 同样加载修复后的 TLS 依赖；不重新 bootstrap 或清除数据库。

后续修改 Gateway hostname 时，保持两端配置一致，重启 Daemon 并重新打开 Firefox 即可；不要求重复重启 Helper。
