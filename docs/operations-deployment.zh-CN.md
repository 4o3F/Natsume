# Natsume 从零部署与运维手册

本文从创建 CA 开始，完成 Ubuntu 24.04 Server 安装和从源码构建 Client Deb，并说明包外输入交接、配套样机注册验收、备份及故障处理。Server 使用 Release Deb，Client 仅构建 Deb，不涉及 ISO 制作。

## 0. 适用范围与执行顺序

核对基线：**Natsume v2.1.0、Ubuntu 24.04 LTS amd64**。Client 运行验收以完成配套 GNOME/GDM、X11 和 Home 集成的样机为前提；其集成要求单独链接，不在本文制作镜像。

PKI 生成、部署文件准备、Server 安装、管理 API 调用和 Client Deb 构建都在同一台 Ubuntu 24.04 Server 上执行。使用同一个具有 sudo 权限的管理员账号，以下 $HOME 均指该账号的 Home；需要服务用户执行的命令会显式使用 sudo -u natsume-server。各节环境变量在同一终端中沿用，换终端后按对应步骤重新设置。

只有第 7 节明确标注的 Client 安装/运行检查在比赛机执行，Web/Firefox 界面操作在实际使用的浏览器中完成。不要把整篇文档当成一个脚本执行。

| 执行位置 | 职责 |
| --- | --- |
| Server 上的管理员账号 | 生成 PKI、准备配置、在本机复制部署输入、调用管理 API、构建 Client Deb |
| Server 上的 natsume-server 服务用户 | 初始化数据库、运行服务、读取在线签发密钥和业务数据库 |
| Client 比赛机 | 安装后生成本机身份、注册、绑定工位、运行比赛会话 |

加密的 CA 私钥原件保存在 Server 管理员的受限 PKI 目录。仅将 Server TLS key 和 Local Origin CA 在线签发 key 复制到服务私有目录；Control CA 私钥不交给 natsume-server 服务用户。CA 私钥不进入源码、Deb 或 Client 的部署输入。

执行顺序：

1. 确定地址、域名和期限。
2. 创建两套 CA、Server TLS leaf，导出所需格式。
3. 安装 Server，部署完整配置和密钥，运行 bootstrap 初始化数据库。
4. 导入比赛数据，从源码构建 Client Deb 并准备包外输入。
5. 如需联调，在配套样机安装后按窗口状态完成自动或人工注册审批，再绑定并验收。
6. 交付包与配置；联调/注册结束后关闭窗口，归档并备份。

Server 的完整 config.toml 包含 DOMjudge 上游地址。bootstrap 创建/迁移全部业务表，并在同一事务中初始化首个管理员和 Runtime Config，无需手工写数据库。

本手册提供操作流程，不代表目标服务器、Client 桌面、显卡、DOMjudge 和整个现场链路已经验收。

阅读入口：[部署参数](#1-准备部署参数和外部服务) · [创建 CA](#2-创建-ca-和-server-tls-证书) · [安装 Server](#4-在服务器安装-server) · [构建 Client Deb](#6-从源码构建-client-deb) · [部署交接与联调](#7-client-部署输入与首次联调) · [备份维护](#8-日常维护备份和恢复)。

## 1. 准备部署参数和外部服务

### 1.1 填写部署清单

以下均为演示值，部署前按实际现场修改。192.0.2.10 是文档示例地址，不能直接使用。

| 参数 | 演示值 | 约束与用途 |
| --- | --- | --- |
| Server IP | 192.0.2.10 | 比赛机实际可达的固定 IP，Client 只接受 IP literal |
| HTTPS 端口 | 8443 | Server 监听、Client 配置、防火墙保持一致 |
| Gateway hostname | domjudge | 每台比赛机解析到自己的 127.0.0.1 和 ::1 |
| DOMjudge 上游 origin | https://judge.contest.example | 真实 DOMjudge，不能指向 Client loopback |
| contest end | 2026-12-06T10:00:00Z | 按实际赛事填写 UTC 比赛结束时间 |
| Gateway not after | 2026-12-08T10:00:00Z | 至少覆盖 contest end 后 **86400 秒**，部署时尚未过期 |
| 版本 | 2.1.0 | 两端 Deb 与镜像集成要求配套 |

本文选择 domjudge，以匹配当前镜像默认的 https://domjudge/ 主页和书签。它只是本机 Gateway 名称，**不是实际上游地址，也不意味着预置了某个 CA**。若改用 gateway.contest.example 等名字，须同步两端配置、Client hosts、Firefox 主页和书签。

Client 使用程序内固定的命名空间从硬件证据派生设备 ID，无需生成或配置站点 UUID。两端必须匹配 Gateway hostname 和两份 CA；比赛结束时间和 Gateway 期限只属于 Server。

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

Natsume 不安装 DOMjudge。继续前需有工作的 HTTPS DOMjudge 和比赛账号，并核对配套登录约定：Gateway 只对 /login 注入 X-DOMjudge-Login 和 Base64 编码的 X-DOMjudge-Pass。实际 DOMjudge 必须支持此约定；普通页面可访问不证明自动登录可用。

上游必须是 canonical HTTPS origin，例如 https://judge.contest.example 或 https://judge.contest.example:8444，不带结尾斜杠、路径、用户名密码、query 或 fragment。不能填写 /domjudge 或 /api 地址，需要在根路径提供配套页面。

Caddy 用**系统信任**校验上游 TLS。若 DOMjudge 使用额外私有 CA，按 7.3 节给 Client 安装该公共 CA；Natsume 的两份 CA 不会自动使任意上游证书受信任。

## 2. 创建 CA 和 Server TLS 证书

### 2.1 明确材料归属

| 材料 | 公共证书接收方 | 私钥位置与用途 |
| --- | --- | --- |
| Control CA | Server、Client、访问 Panel 的管理员浏览器 | Server 管理员的 PKI 目录保存加密私钥，直接签发 Server TLS leaf |
| Local Origin CA | Server、Client 系统和 Firefox | 加密原件保存在同一 PKI 目录；在线副本放服务私有目录，动态签发 Gateway leaf |
| Server TLS leaf/key | leaf 通过 TLS 握手提供 | Server 私有目录，用于 HTTPS/WSS |
| server-root.key | 不分发 | bootstrap 在 Server 生成，保护数据库中的业务秘密，**不是 CA key** |

本流程使用 ECDSA P-256 和 SHA-256，两个自签根直接签发 leaf。不要增加中间 CA：当前 Server TLS 配置读取单个 leaf DER，Gateway grant 也只携带单个 leaf。

### 2.2 建立受控工作目录

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

### 2.3 创建 Control CA

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

### 2.4 创建 Local Origin CA，导出在线材料

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

origin-ca-key.pk8 是**未加密 PKCS#8 DER**，这是 Server 的读取格式；只能作为在线签发密钥部署到 Server 受限目录。后缀不会转换内容。参数见 [OpenSSL pkcs8](https://docs.openssl.org/3.0/man1/openssl-pkcs8/)。

Local Origin CA 有效期必须覆盖 Gateway not after，不能只把 TOML 期限改到根证书有效期之外。程序不替运维自动续期 CA。

### 2.5 签发含 Server IP SAN 的 TLS leaf

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

### 2.6 验证证书、编码和公钥匹配

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

### 2.7 用现有 Local Origin CA 签发 DOMjudge 上游证书（可选）

DOMjudge 可以使用第 2.4 节的 Local Origin CA（Gateway CA）签发的服务器证书。它使用独立的叶子私钥；CA 私钥仍保存在 Natsume Server 的 PKI 目录。以下签发操作继续在 Natsume Server 上执行，先将 IP 换成实际 DOMjudge 上游地址：

~~~bash
umask 077
NATSUME_PKI_DIR="$HOME/natsume-pki-2026"
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

签发时输入既有 CA 私钥口令。有效期不得超过该 CA；若使用域名访问，SAN 改为 DNS:实际上游域名，并用 openssl verify -verify_hostname 核验。不要把 Client loopback 的 Gateway hostname 当成上游地址。

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

预期全部归属 root:root，目录 700、证书 644、私钥 600。Nginx 使用 PEM 编码的证书和私钥，本节生成的两个 .pem 文件可以直接使用，文件名不必改成 .crt 或 .key；见 [Nginx SSL 配置说明](https://nginx.org/en/docs/http/ngx_http_ssl_module.html#ssl_certificate)。

用 sudoedit 编辑已启用的 DOMjudge 站点配置，在现有 server 块中增加或调整以下 TLS 指令，保留原有的页面、PHP 和反向代理配置：

~~~nginx
listen 443 ssl;
ssl_certificate     /etc/nginx/tls/domjudge-tls-leaf.pem;
ssl_certificate_key /etc/nginx/tls/domjudge-tls-key.pem;
~~~

证书和私钥由 root master 进程加载，因此配置检查也须使用 sudo。普通用户直接执行 nginx -t 可能因无法读取受限目录或私钥而报 Permission denied；这不表示证书格式错误。检查成功后再重载已运行的服务：

~~~bash
sudo nginx -t && sudo systemctl reload nginx
~~~

若 sudo nginx -t 仍然报错，根据错误中的路径检查文件和各级目录的权限。容器或非 root master 部署按其实际挂载路径和运行用户设置读取权限。启用 HTTPS 后，从 Client 验证：

~~~bash
curl --fail --show-error --cacert /etc/natsume/trust/local-origin-ca.crt \
  https://192.0.2.20/
~~~

随后将 Server config.toml 的 [runtime].domjudge_origin 设置为实际 HTTPS origin。第 7.3 节仍需把 Local Origin CA 加入 **Client 系统信任库**，并在信任库变更后重启 natsume-device-daemon.service，使 Caddy 重新加载信任。仅导入 Firefox 不能替代 Caddy 的上游 TLS 信任。完成后再用不带 --cacert 的 curl 验证系统信任。

## 3. 整理本机部署目录并下载 Release

### 3.1 整理部署目录

**执行位置：Server，继续使用生成 PKI 的管理员账号。**建立 Git 仓库外受控目录：

~~~bash
umask 077
NATSUME_DEPLOY_DIR="$HOME/natsume-deploy"
mkdir -p "$NATSUME_DEPLOY_DIR"/{public,server,client,packages}
chmod 0700 "$NATSUME_DEPLOY_DIR" "$NATSUME_DEPLOY_DIR/server"
~~~

以下文件已在本机 $HOME/natsume-pki-2026 中生成，第 4.1 节将其直接复制到部署目录。加密私钥原件和签发记录继续保留在 PKI 目录：

| PKI 路径 | 部署目录下路径 | 最终接收方 |
| --- | --- | --- |
| public/control-ca.crt | public/control-ca.crt | Server、Client、管理员浏览器 |
| public/local-origin-ca.crt | public/local-origin-ca.crt | Server、Client |
| server/origin-ca.der | server/origin-ca.der | Server |
| server/origin-ca-key.pk8 | server/origin-ca-key.pk8 | **仅 Server，秘密** |
| server/server-tls-leaf.der | server/server-tls-leaf.der | Server |
| server/server-tls-key.pk8 | server/server-tls-key.pk8 | **仅 Server，秘密** |

只复制表中列出的文件，不把 PKI 的 private/ 整目录交给服务。Client 部署方只接收其配置和公共证书，不接收 server/ 目录。

### 3.2 下载并验证 Server 包

在 Server 本机下载 [v2.1.0 Release](https://github.com/4o3F/Natsume/releases/tag/v2.1.0) 中的 Server 包（需先完成该版本发布）：

~~~bash
cd "$NATSUME_DEPLOY_DIR/packages"
NATSUME_RELEASE_URL='https://github.com/4o3F/Natsume/releases/download/v2.1.0'

curl --fail --location --remote-name "$NATSUME_RELEASE_URL/natsume-server_2.1.0_amd64.deb"
curl --fail --location --remote-name "$NATSUME_RELEASE_URL/SHA256SUMS"
test -s natsume-server_2.1.0_amd64.deb
sha256sum --check --ignore-missing SHA256SUMS
dpkg-deb --field natsume-server_2.1.0_amd64.deb Package Version Architecture
~~~

Server 校验须为 OK，版本/架构为 2.1.0/amd64。Release 的 SHA256SUMS 同时列出两种包，此处用 --ignore-missing 跳过未下载的官方 Client 包，Client 在第 6 节从源码构建。归档 Deb 和 checksum；同渠道 checksum 用于核对下载字节，不能代替对发布来源的信任。

## 4. 在服务器安装 Server

### 4.1 在本机复制 PKI 输入

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

Server Deb 直接使用第 3.2 节已下载并校验的本地文件。下一节安装软件包，再将这些材料复制到最终服务路径。

### 4.2 安装软件包

**执行位置：Server 管理员终端。**

~~~bash
dpkg --print-architecture
sudo apt-get update
sudo apt-get install --yes ca-certificates openssl curl sqlite3 python3
sudo apt-get install --yes "$NATSUME_DEPLOY_DIR/packages/natsume-server_2.1.0_amd64.deb"
dpkg-query -W natsume-server
getent passwd natsume-server
~~~

预期架构 amd64、包已配置、服务账号存在。安装脚本初始化服务账号和目录等，不生成配置/CA、不初始化管理员、不自动启用或启动服务；缺少部署文件时允许通用包预装。

### 4.3 安装证书、密钥和完整配置

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
root_key = "/var/lib/natsume-server/keys/server-root.key"
organization_logos = "/var/lib/natsume-server/organization-logos"

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

keys 目录应为 natsume-server:natsume-server 700，文件 600。启动时进程还会验证完整配置、编码、证书/私钥匹配和 Origin CA 一致性，以上不是全部验证。

### 4.4 初始化数据库

首次部署，在交互 TTY 运行：

~~~bash
sudo -u natsume-server -- /usr/bin/natsume-server bootstrap
~~~

按提示输入管理员登录名及两次密码。必须使用服务用户，不直接以 root 运行，不放入 Deb 安装脚本或自动化任务。bootstrap 会创建/迁移完整数据库 schema、生成缺失的 server-root.key，并在同一事务中创建首个管理员及写入配置中的 DOMjudge 上游，成功后退出。

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

### 4.5 启动并验证 TLS

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

应 HTTP 成功、证书验证成功、ALPN 为 http/1.1。Client 的远程可达性在第 7 节样机验收时验证。不要用 curl -k。health 只说明进程存活，不代替业务验收。


## 5. 管理入口与比赛数据

### 5.1 信任 Control CA，登录 Panel

在用于访问 Panel 的管理员 Firefox 中：设置 → 隐私与安全 → 证书 → 查看证书 → 证书颁发机构 → 导入本机生成、经过指纹核对的 control-ca.crt，允许识别网站。访问 https://实际ServerIP:8443/，应无证书警告，使用 bootstrap 管理员登录。

这是管理员浏览器访问 Server 的 Control CA；比赛机浏览器访问本机 Gateway 使用 Local Origin CA。

### 5.2 导入完整队伍名单

v2.1.0 使用完整 XLSX 名单导入，旧 CSV 入口已移除。从旧版升级前，先完成第 8.5 节的配套升级步骤。

在 Web **Preparation** 点击 **Download Excel template**，下载 `Teams` 工作表模板。人工把报名表整理为一行一队，按[字段规则](../crates/roster/README.md#excel-模板)填写九列：学校中英文名、country、account、password、seat、队伍中英文名、category。所有单元格按文本填写，保留座位前导零；不填写公式。学校与队伍的中英文名各至少填写一种，country 留空默认为 CHN。

填写 DOMjudge 队伍账号和当前密码；account、seat 各自唯一。学校编号由 Server 生成，无需预填。文件包含密码，在 Server 受控目录保存时设为 0600，不提交 Git。每次上传完整名单；热身转正式赛只修改 password，其余各行也一并上传。

1. 选择 XLSX（最大 8 MiB），点击 **Create preview**。
2. 核对 **Team changes / School changes / Preview school logos**，检查队伍、中英文名称、类别和 INST ID。
3. 核对账号新增／移除、**Passwords changed**、**Seat changes / Mapping changes** 和 **Binding impacts**。这里只显示密码变化的账号名。
4. 删除已绑定的座位会阻止提交，需要先通过 **Bindings** 解绑；其他资料变化会列出受影响设备，绑定保留在原座位。
5. 点击 **Commit import** → **Confirm commit**。整份名单原子生效；完全相同的文件不改业务数据，只有实际改密账号推进凭据 revision。队伍资料变化不切换设备前台会话。
6. 在 **Seats / Accounts** 核对数量与映射。刷新或退出后需要 **Discard preview** 再上传；普通页面内导航仍保留审核过的文件。

v2.1.0 提供 Logo 目录、网页观测和完整 DOMjudge ZIP。配套 Client 在 waiting 页面显示队伍／学校／Logo，断网时保留上次资料并显示离线标识。

在同一份 Server config.toml 的 `[storage]` 中设置 `organization_logos`（示例已列出）。当前源码的 Server Deb 在安装／重装时自动创建默认目录 `/var/lib/natsume-server/organization-logos`，归属 `root:natsume-server`、权限 `0750`，保留已有图片。自定义路径由部署方创建并授予服务用户读取／遍历权限。首次配置路径需要重启 Server；以后补图或替换源文件无需重启或重新导入名单。

已发布的 v2.1.0 标签尚未包含自动建目录的修正，可先执行下面这条幂等命令补建；使用包含修正的新包时无需手动创建默认目录：

```bash
sudo install -d -o root -g natsume-server -m 0750 \
  /var/lib/natsume-server/organization-logos
```

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

提交名单后，管理员点击 **Download DOMjudge ZIP**。即使存在待审核名单，下载内容也只取当前已提交的完整名单；设备分页、是否绑定均不影响结果。ZIP 包含：

- `groups.json`、`organizations.json`、`teams.json`、`accounts.yaml`；
- `README.md`（导入顺序、命令、图片部署和学校／源文件对照）；
- `logos/INST-xxx.png`（当前名单所有可用校徽，按实际内容转为 PNG）。

DOMjudge 使用新版 JSON／YAML 入口按 groups → organizations → teams → accounts 顺序导入；将 `logos/` 内容复制到 DOMjudge 的 `webapp/public/images/affiliations/`，保留 INST 文件名并赋予 Web 服务读权限。详见包内 README。不使用 legacy TSV。缺图／歧义会在 README 列明，其余内容照常导出；匹配到的图片损坏、不可读或超限则整包失败，页面显示学校和文件原因。

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

**执行位置：同一台 Ubuntu 24.04 amd64 Server，使用管理员账号。**在本机安装编译依赖，只构建 Client 的三个 Rust 程序和 Deb，无需 Node/pnpm/Web 编译环境。源码放在管理员 Home 的独立目录，不放入 PKI 或部署材料目录：

~~~bash
sudo apt-get update
sudo apt-get install --yes build-essential pkg-config curl git ca-certificates \
  gettext-base libfontconfig1-dev libgl1-mesa-dev libudev-dev \
  python3 binutils xz-utils
mkdir -p "$HOME/src"
cd "$HOME/src"
git clone --branch v2.1.0 --depth 1 https://github.com/4o3F/Natsume.git Natsume-v2.1.0
cd Natsume-v2.1.0
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

后续所有构建命令在仓库根目录的同一终端运行。提前检查磁盘与内存，Rust/Slint release 编译需要容纳源码、依赖和中间产物；内存不足时可给 cargo build 增加 -j 2 限制并发。

### 6.2 下载并校验 Caddy 和 nFPM

v2.1.0 固定 Caddy 2.11.4、nFPM 2.47.0。版本和摘要由 packaging/client/caddy.version、caddy.archive.sha256、caddy.sha256 及 packaging/nfpm.version、nfpm.sha256 管理。

~~~bash
NATSUME_SOURCE_DIR="$PWD"
NATSUME_TOOL_DIR="$(mktemp -d)"

curl --fail --location \
  https://github.com/caddyserver/caddy/releases/download/v2.11.4/caddy_2.11.4_linux_amd64.tar.gz \
  --output "$NATSUME_TOOL_DIR/caddy_2.11.4_linux_amd64.tar.gz"
curl --fail --location \
  https://github.com/goreleaser/nfpm/releases/download/v2.47.0/nfpm_2.47.0_Linux_x86_64.tar.gz \
  --output "$NATSUME_TOOL_DIR/nfpm_2.47.0_Linux_x86_64.tar.gz"

(
  cd "$NATSUME_TOOL_DIR"
  sha256sum --check "$NATSUME_SOURCE_DIR/packaging/client/caddy.archive.sha256"
  sha256sum --check "$NATSUME_SOURCE_DIR/packaging/nfpm.sha256"
)

tar -xzf "$NATSUME_TOOL_DIR/caddy_2.11.4_linux_amd64.tar.gz" -C "$NATSUME_TOOL_DIR" caddy
tar -xzf "$NATSUME_TOOL_DIR/nfpm_2.47.0_Linux_x86_64.tar.gz" -C "$NATSUME_TOOL_DIR" nfpm

(
  cd "$NATSUME_TOOL_DIR"
  sha256sum --check "$NATSUME_SOURCE_DIR/packaging/client/caddy.sha256"
)
"$NATSUME_TOOL_DIR/caddy" version
"$NATSUME_TOOL_DIR/nfpm" --version
~~~

所有摘要检查须为 OK，版本分别匹配。任何一步失败都先解决，不继续打包。这里同时校验 Caddy 压缩包和解出的二进制，使用仓库锁定的官方模块组合。

### 6.3 编译 Client 三个程序

使用独立 target 目录，避免误取旧构建产物：

~~~bash
export CARGO_TARGET_DIR="$NATSUME_SOURCE_DIR/target/client-deb"
cargo build --release --locked \
  -p natsume-device-daemon \
  -p natsume-privileged-helper \
  -p natsume-session-agent

ls -lh "$CARGO_TARGET_DIR/release/natsume-device-daemon" \
  "$CARGO_TARGET_DIR/release/natsume-privileged-helper" \
  "$CARGO_TARGET_DIR/release/natsume-session-agent"
~~~

这是生产构建，不增加 --all-features 或测试特性。编译不需要任何 CA、站点配置、Server 私钥或设备身份。

### 6.4 使用现有 manifest 打包

下面与仓库 package-client recipe 使用同一 manifest，直接调用工具，不要求额外安装 just：

~~~bash
export VERSION='2.1.0'
export ARCH='amd64'
export RUST_RELEASE_DIR="$CARGO_TARGET_DIR/release"
export CADDY_BIN="$NATSUME_TOOL_DIR/caddy"

python3 packaging/check-image-inputs.py
mkdir -p dist/packages
envsubst '$ARCH $VERSION $RUST_RELEASE_DIR $CADDY_BIN' \
  < packaging/client/nfpm.yaml > "$NATSUME_TOOL_DIR/client.nfpm.yaml"
"$NATSUME_TOOL_DIR/nfpm" package --packager deb \
  --config "$NATSUME_TOOL_DIR/client.nfpm.yaml" --target dist/packages/

dpkg-deb --field dist/packages/natsume-client_2.1.0_amd64.deb Package Version Architecture
python3 packaging/check-image-inputs.py --deb dist/packages/natsume-client_2.1.0_amd64.deb
(
  cd dist/packages
  sha256sum natsume-client_2.1.0_amd64.deb > natsume-client_2.1.0_amd64.deb.sha256
  sha256sum --check natsume-client_2.1.0_amd64.deb.sha256
)
~~~

交付产物为 **dist/packages/natsume-client_2.1.0_amd64.deb** 及本次生成的 checksum。check-image-inputs 检查 Deb 内附带的桌面集成交接材料，并不构建 ISO，也不在 Server 上创建 Client 账号或启动 Client 服务。

不传入 SITE_CONFIG、CONTROL_CA_CERT 或 LOCAL_ORIGIN_CA_CERT；通用 Deb 不包含测试/正式 CA 和部署配置。它包含程序、Caddy、包所属运行文件、配置示例及完整交接目录。

自建包摘要不一定与官方 Release 相同，应配套使用**本次生成的 checksum**。如果改过源码，选择有区别的 Debian 版本，同步命令中的文件名，记录 Git SHA、改动、工具版本和日志，不冒充官方同名产物。

## 7. Client 部署输入与首次联调

本节用于交接构建产物，以及在**已完成配套桌面集成的样机**上联调。普通 Ubuntu 只安装 Deb，不会自动完成 waiting/teams、PAM/GDM、Kiosk、Home 模板及其 mount 配置；完整前提见包内 /usr/share/natsume/image-integration/ 和[镜像实施要求](../packaging/image/integration.md)。本文不展开 ISO 制作。

### 7.1 准备包外输入并交接

在 Server 本机创建完整 Client config.toml，替换实际 IP、端口和 Gateway hostname：

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

不交付 Server 私钥、CA 私钥、设备身份或 token。Client 无单独 site.toml，Server 的日期和私有材料路径不进入 Client 配置。

若部署方先预装 Deb，可稍后再提供配置与两个 CA；首次启动前必须补齐。包脚本只做存在性/非空/可读等基础检查，不生成或改写这些文件。

### 7.2 在配套 Client 样机安装并落地配置

把 Deb、它的 checksum、config.toml、两个公共 CA 放到样机管理员的 ~/natsume-client-install/。先校验包，然后安装；文件落地前保持 Daemon 停止：

~~~bash
cd "$HOME/natsume-client-install"
sha256sum --check natsume-client_2.1.0_amd64.deb.sha256
sudo apt-get update
sudo apt-get install --yes "$PWD/natsume-client_2.1.0_amd64.deb"

sudo install -d -o root -g root -m 0755 /etc/natsume /etc/natsume/trust
sudo install -o root -g root -m 0644 config.toml /etc/natsume/config.toml
sudo install -o root -g root -m 0644 control-ca.crt /etc/natsume/trust/control-ca.crt
sudo install -o root -g root -m 0644 local-origin-ca.crt /etc/natsume/trust/local-origin-ca.crt
~~~

检查配置和证书指纹与 Server 部署一致。用 sudoedit /etc/hosts 添加以下两行；若已有该名称的记录，先合并，不留下指向上游的冲突记录：

~~~text
127.0.0.1 domjudge
::1 domjudge
~~~

Gateway hostname 改名时同步这里和 Firefox 主页/书签。真实上游 judge.contest.example 不能指向 loopback。

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
getent ahostsv4 domjudge
getent hosts domjudge
sudo systemctl enable --now natsume-privileged-helper.service natsume-device-daemon.service
systemctl status natsume-privileged-helper.service natsume-device-daemon.service --no-pager
sudo journalctl -u natsume-privileged-helper.service -u natsume-device-daemon.service -b -n 150 --no-pager

curl --fail --show-error --http1.1 --tlsv1.3 \
  --cacert /etc/natsume/trust/control-ca.crt \
  https://192.0.2.10:8443/api/v2/health
curl --show-error --silent --output /dev/null --write-out '%{http_code}\n' \
  https://judge.contest.example/
~~~

预期版本 2.1.0、模板只读 SquashFS、Gateway 仅解析到 loopback，Server 和上游 TLS/HTTP 正常。第二个 curl 使用系统信任，核对 Caddy 的上游信任来源。Caddy 由 Daemon 依赖管理，Agent 由官方 Kiosk 用户服务管理，不单独增加另一个启动入口。

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

需要逐台人工审核时，让窗口保持 **Closed**。Web **Enrollment** 仍会出现 pending review；逐台核对物理工位、硬件 ID、证据质量、Daemon/Agent 版本和候选公钥，再点 **Approve**；未知设备使用 **Deny** 并排查。不要按列表顺序盲目审批。

Server 每次重启窗口恢复关闭，新请求改为人工审批。Client 完成身份初始化和连接后才开始注册；关闭窗口不代替已注册设备的 Revoke。

样机显示 **Bind workstation / Enter your seat code** 时输入 XLSX 中的工位码，如 A-01。在 **Bindings / Seats / Devices** 核对工位、设备和账号一致。工位不存在/被占时修正映射，不删除设备身份文件重试。

新设备默认保持 waiting，绑定成功后等待管理员执行 **Show contest desktop**。绑定和重启不会改写已有的前台目标；已有设备若要继续等待，先在 **Targets** 执行 **Show waiting screen**。重启先进入 waiting，重新连接且桌面依赖就绪后按 Server 保存的目标恢复前台。

Client 自动生成 Gateway key/CSR，由 Server 签发 leaf 并应用配置，运维不分发每台 Gateway 私钥或账号密码文件。

### 7.6 验收实际桌面和 DOMjudge

**Targets** 选样机 → **Show contest desktop**。请求成功不等于已完成，检查当前报告：

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

仅监听 127.0.0.1:443 和 [::1]:443，TLS 和业务响应正常。未完成注册/配置时可能无 listener 或 503，不能作为最终通过。

在 **teams 比赛桌面的 Firefox** 访问 https://domjudge/，必要时进入 /login：

1. 无证书警告，地址仍为 Gateway hostname。
2. 登录显示绑定工位的正确 DOMjudge 账号。
3. 页面、静态资源、提交题目和所需实时更新正常。
4. 无跳转绕开 Gateway、跨工位串号等问题。

真实 DOMjudge 的登录头、重定向、Cookie、Host 和资源行为需这里验收；HTTPS 成功只覆盖一部分。不要把管理员浏览器 profile 制作成比赛模板。

测试 **Show waiting screen** → **Show contest desktop**，确认前台切换及比赛进程/Home 保留。在比赛终端运行 echo "$XDG_SESSION_TYPE" 应为 x11，不能用 SSH shell 的值判定图形会话。

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

Web 检查各设备实际状态和报告时间。不要公开原始数据库、cookie、私钥或运行中的 Caddy 配置，READY 配置包含访问凭据。普通 tracing/OTLP 是诊断信息，不是持久业务审计。

### 8.2 Server 冷备份

维护窗口停止唯一 Server 进程，确认没有手动运行的 serve 后，获取 SQLite 一致副本，再与同一时间点的 key/config 一起备份。目标目录不得放在被归档的状态目录内：

~~~bash
sudo systemctl stop natsume-server.service
systemctl is-active natsume-server.service
~~~

预期 inactive（该命令非零退出）。确认后继续：

~~~bash
NATSUME_BACKUP_DIR="/var/backups/natsume/$(date -u +%Y%m%dT%H%M%SZ)"
NATSUME_PKI_DIR="$HOME/natsume-pki-2026"
sudo install -d -o root -g natsume-server -m 0710 /var/backups/natsume
sudo install -d -o natsume-server -g natsume-server -m 0700 "$NATSUME_BACKUP_DIR"
sudo -u natsume-server sqlite3 /var/lib/natsume-server/natsume.db \
  ".backup '$NATSUME_BACKUP_DIR/natsume.db'"
sudo chown root:root "$NATSUME_BACKUP_DIR" "$NATSUME_BACKUP_DIR/natsume.db"
sudo chmod 0600 "$NATSUME_BACKUP_DIR/natsume.db"
sudo install -o root -g root -m 0600 /dev/null "$NATSUME_BACKUP_DIR/config-and-keys.tar"
sudo tar --numeric-owner --acls --xattrs -cpf "$NATSUME_BACKUP_DIR/config-and-keys.tar" -C / \
  etc/natsume-server etc/natsume/trust var/lib/natsume-server/keys
sudo install -o root -g root -m 0600 /dev/null "$NATSUME_BACKUP_DIR/pki.tar"
sudo tar -cpf "$NATSUME_BACKUP_DIR/pki.tar" -C "$NATSUME_PKI_DIR" .
sudo sqlite3 "$NATSUME_BACKUP_DIR/natsume.db" 'PRAGMA integrity_check;'
sudo tar -tf "$NATSUME_BACKUP_DIR/config-and-keys.tar"
sudo systemctl start natsume-server.service
~~~

完整性应为 ok。源数据库由服务用户打开，避免产生 root 所有的运行状态文件；备份完成后目录和文件归 root 管理。归档包含 DB 一致副本、vault root、Origin CA、TLS 和完整公共配置，缺一不可。使用 SQLite .backup 避免仅复制主库漏掉 WAL，依据 [SQLite Online Backup](https://www.sqlite.org/backup.html)。

本流程的 CA 在 Server 本机生成，因此 pki.tar 另行保存管理员 PKI 目录中的加密私钥原件、证书和签发记录，供后续续签使用。备份期间暂停 PKI 签发操作；私钥口令仍由密码管理器保管。

用现场备份系统加密复制到独立存储，同时保存匹配 Deb。server-root.key 丢失等于 vault 秘密不可恢复，只备份数据库或重新 bootstrap 一个 key 都不能恢复旧凭据。

### 8.3 恢复与管理员密码重置

恢复先在隔离环境演练，避免两台 Server 同时服务同一批 Client：

1. 安装相同版本 Deb，停服并保存故障现场快照。
2. 将 config-and-keys.tar 解包到受限 staging 目录，核对路径和材料。
3. 用备份 DB 和完整旧 key/config 替换目标，不能混合新旧数据库、WAL、SHM；保留故障副本后移走旧 DB 配套文件。
4. 换主机时按目标服务 UID/GID 重设状态文件 owner，keys 0700、秘密文件 0600、公共配置和 CA root:root 0644。
5. 检查数据库完整性和证书匹配，启动并按 4.5、7.6 节验收已注册设备重连。**不要运行 bootstrap。**

pki.tar 恢复到 Server 管理员的 PKI 目录，保持目录 0700、私钥文件 0600 和管理员归属，供后续签发使用；它不解包到 natsume-server 服务私有目录。

只重置管理员密码时，在可信 TTY 和维护窗口运行：

~~~bash
sudo systemctl stop natsume-server.service
sudo -u natsume-server -- /usr/bin/natsume-server reset-operator-password
sudo systemctl start natsume-server.service
~~~

输入目标登录名和两次新密码。该管理员已有会话失效，不创建账号、不改 vault root。不要直接编辑密码 hash。

### 8.4 配置、证书和版本更新

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

Client 升级不会自动应用新 /usr/share/natsume/image-integration/。升级/回退按[镜像维护要求](../packaging/image/integration.md#11-维护与回退)，不能只降级 Deb 然后沿用不兼容状态。不要克隆运行过机器的身份、Control/Gateway key、Enrollment 或 Home reset 状态到其他机器。

旧版使用站点 UUID 的 Client 不能通过删除配置项完成原地升级：本版的硬件 ID 派生规则和 identity.json 格式均已改变。维护时先完成 Home 恢复、退出受管会话并备份所需数据，再在 Server 解除旧 Binding、Revoke 旧设备，从干净镜像重新部署，按 7.5 节重新审批和绑定。不要只删除 identity.json 或混用旧身份、Control/Gateway 凭据。Daemon 与 Helper 必须配套更新；未运行过的 Client 可直接使用本文的新配置。

<a id="upgrade-v2-1-0"></a>
### 8.5 从 v2.0.4 配套升级到 v2.1.0（Breaking Change）

v2.1.0 是不兼容更新。Server、Device Daemon、Helper、Session Agent 与 Web 必须配套；控制连接使用 `natsume.control.v3`，本地 Agent 注册使用协议 3。旧 Client 不能继续连接新 Server。新版绑定目标必须包含队伍／学校资料，因此须先补全名单，再让设备重连。

1. 在维护窗口完成已开始的 Home reset／会话维护，保存比赛数据，暂停新操作。按[镜像维护流程](../packaging/image/integration.md#11-维护与回退)退出受管会话并停止 Client 控制连接；确认 Server 已显示设备离线。
2. 按第 8.2 节保存一致的数据库、密钥与配置冷备份，另行保存 Logo 目录、完整名单和匹配旧包。备份步骤结束后再次停止 Server，保持停止状态进行软件包升级。
3. 安装第 3.2 节校验过的 `natsume-server_2.1.0_amd64.deb`。在已有 `/etc/natsume-server/config.toml` 的 `[storage]` 中补上必填绝对路径 `organization_logos`，参考第 4.3 节。保留原数据库和全部密钥，**不要重新 bootstrap，也不要删除数据库或设备身份**。
4. 显式启动 Server；`serve` 在启动时执行数据库迁移，保留已有 Device、Enrollment、Account、Vault、Binding 和 Session 目标。迁移会移除旧格式的未提交导入候选，须重新上传完整 XLSX。
5. 保持 Client 停止，通过新版 Panel 按第 5.2 节上传完整名单并提交。账号与座位须与现有数据一致；不准备改密时填写原密码。核对学校、队伍资料和已有绑定，准备 Logo。没有 Logo 允许使用默认图；缺少队伍资料的已绑定账号不能生成有效控制目标。
6. 安装配套 `natsume-client_2.1.0_amd64.deb`，按镜像交接流程检查并应用随包配置差异。包会提供 Noto CJK 字体依赖和展示目录；保留原 Client 身份、Control/Gateway 凭据及 Home 维护状态。确认 `/var/lib/natsume-display` 为 natsume 所有、0755，waiting 只能读取图片。
7. 先恢复一台测试设备，再逐批恢复。核对设备 Online、原绑定／座位、waiting 队伍资料、Logo 和五项资源状态；缺图不等于离线。检查切换、断线缓存、解绑后不恢复旧队伍，并完成[配套验收](../packaging/image/acceptance.md#新版-waiting-展示的配套验证)。

需要回退时，恢复同一维护点的旧版本 Server/Client、数据库与密钥／配置备份；不能把新数据库与旧二进制混用。更早的站点 UUID Client 仍按第 8.4 节处理，不套用本节的身份保留流程。最终镜像与容量验收独立记录，发布标签本身不表示这些验收已经完成。

## 9. 故障定位表

| 现象 | 先检查 | 处理方向 |
| --- | --- | --- |
| unit skipped | 三个必需输入、systemctl status | 部署完整配置和 CA 后启动 |
| Server TLS identity invalid | DER/PKCS#8、公钥匹配、keys 0700/files 0600 | 按 2.6 节检查，不把 PEM 改后缀冒充 DER |
| Origin CA mismatch | public PEM 解码与 origin-ca.der | 必须同一证书且私钥匹配，不能只比较 CN |
| Nginx 检查证书报 Permission denied | 是否以普通用户执行 nginx -t，证书路径及目录权限 | 按 2.7 节设置权限，用 sudo nginx -t 检查成功后再重载 |
| Client TLS 失败 | 时钟、Control CA、IP SAN、端口 | 从 Client 用不带 -k 的 curl/openssl 检查 |
| 配置读取失败（NotFound / PermissionDenied） | 报错中的配置路径、服务用户读取权限 | 确认 /etc/natsume-server/config.toml 存在且 natsume-server 可读 |
| missing required field | 报错指出的 section 或字段 | 对照 4.3 节补齐完整配置 |
| TOML syntax or field type | 报错的行列、引号和值类型 | 修正 TOML 后重试 |
| runtime.domjudge_origin 校验失败 | 实际上游是否提供 HTTPS，地址是否只包含 origin | HTTP 地址不受支持；使用可用的 HTTPS 上游，不带末尾斜杠或路径 |
| 无 Enrollment review | 窗口、Devices 列表、控制连接与注册握手 | Open 会自动批准，先查 Devices；Closed 应有待审请求，检查 Client/Server 日志 |
| 无法绑定 | XLSX 是否 commit、工位是否存在/被占 | 处理数据与 Binding，不删除设备身份 |
| Firefox 不信任 Gateway | about:policies、Install路径、CA/hostname/期限 | 修正策略并完全重启 Firefox |
| Gateway 连接失败/503 | loopback解析、listener、Gateway/Binding/Runtime | 先完成注册配置，BLOCKED 不能靠关闭 TLS 验证解决 |
| Gateway 502/上游 TLS 失败 | DNS、系统CA、上游端口/证书 | Client 用系统信任测试，额外 CA 按 7.3 节分发 |
| 页面可访问但登录错误 | 工位账号、密码、登录头支持、Cookie/重定向 | 在 teams Firefox 实测 /login，不打印 Caddy 凭据 |
| 持续橙色桌面/无 Agent | waiting Kiosk、GDM/X11、Helper报告 | 独立管理员维护，不旁路再启动一个 Agent |
| contest 不呈现/Home recovery_required | 模板 mount、Home、Binding、会话报告 | 保存现场数据，按镜像恢复流程处理，不自动删 Home |
| 重装/克隆后身份冲突 | 硬件身份、镜像内旧状态 | 干净通用镜像、每台独立身份，按审批迁移 |
## 10. 实施依据

- [Server 命令、配置和私有材料](../server/README.md)、[Server 配置示例](../packaging/server/config.example.toml)、[Client 配置示例](../packaging/client/config.example.toml)。
- [Server 配置解析](../server/src/config.rs)、[TLS 加载](../server/src/tls.rs)、[Gateway CA 签发](../server/src/component/gateway/issuer.rs)。
- [Runtime Config 读取](../server/src/component/runtime.rs)、[数据库 schema](../server/migrations/00000000000001_initial/up.sql)、[注册窗口 API](../server/src/http/handler/provisioning.rs)。
- [构建与发布](../packaging/README.md)、[Client manifest](../packaging/client/nfpm.yaml)、[桌面集成交接](../packaging/image/README.md)、[验收要求](../packaging/image/acceptance.md)。

架构目标由 [architecture.md](architecture.md) 定义。
