#!/bin/bash
set -e

# REMEMBER TO CHANGE API PATH AND AUTHORIZE TOKEN
DOMJUDGE_VERSION="8.3.2"

SCRIPT_PATH="$(realpath "$0" 2>/dev/null || echo "$(cd "$(dirname "$0")" && pwd)/$(basename "$0")")"

sudo sed -i 's/cn.archive.ubuntu.com/mirrors.bfsu.edu.cn/g' /etc/apt/sources.list

STATE_FILE="/opt/domjudge_install_state"

if [ -f "$STATE_FILE" ]; then

    sudo bash /opt/domjudge/judgehost/bin/create_cgroups

    /opt/domjudge/judgehost/bin/judgedaemon

    sudo rm -f "$STATE_FILE"
    sudo rm -f /etc/systemd/system/domjudge-continue.service
    sudo systemctl daemon-reload

    echo "success"
    exit 0
fi

sudo apt update

sudo apt install -y make sudo debootstrap libcgroup-dev lsof \
    php-cli php-curl php-json php-xml php-zip procps \
    gcc g++ openjdk-8-jre-headless openjdk-8-jdk ghc \
    fp-compiler libcurl4-gnutls-dev libjsoncpp-dev \
    libmagic-dev mono-mcs wget

DOMJUDGE_URL="https://www.domjudge.org/releases/domjudge-${DOMJUDGE_VERSION}.tar.gz"

wget "$DOMJUDGE_URL" -O "domjudge-${DOMJUDGE_VERSION}.tar.gz"
tar -zxvf "domjudge-${DOMJUDGE_VERSION}.tar.gz"
cd "domjudge-${DOMJUDGE_VERSION}"

sudo mkdir -p /opt/domjudge
./configure --prefix=/opt/domjudge --with-baseurl=127.0.0.1
make judgehost
sudo make install-judgehost

sudo useradd -d /nonexistent -U -M -s /bin/false domjudge-run || true

sudo cp /opt/domjudge/judgehost/etc/sudoers-domjudge /etc/sudoers.d/

RESTAPI_SECRET_FILE="/opt/domjudge/judgehost/etc/restapi.secret"
echo "default http://<host>/api judgehost <token>" | sudo tee "$RESTAPI_SECRET_FILE"

sudo sed -i 's,http://us.archive.ubuntu.com/ubuntu/,http://mirrors.aliyun.com/ubuntu,g' /opt/domjudge/judgehost/bin/dj_make_chroot

sudo /opt/domjudge/judgehost/bin/dj_make_chroot
sudo sed -i '/^GRUB_CMDLINE_LINUX_DEFAULT=/c\GRUB_CMDLINE_LINUX_DEFAULT="quiet cgroup_enable=memory swapaccount=1 systemd.unified_cgroup_hierarchy=0"' /etc/default/grub
sudo update-grub

sudo touch "$STATE_FILE"

sudo tee /etc/systemd/system/domjudge-continue.service > /dev/null <<EOF
[Unit]
Description=Continue DOMjudge Install after Reboot
After=network.target

[Service]
Type=oneshot
ExecStart=/bin/bash $SCRIPT_PATH
RemainAfterExit=true

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable domjudge-continue.service

sleep 5
sudo reboot