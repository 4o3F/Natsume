#!/bin/bash
set -euo pipefail

# ╔════════════════════════════════════════════════════════════╗
# ║  DOMjudge Judgehost Setup Script                          ║
# ║  Edit the variables below before running                  ║
# ╚════════════════════════════════════════════════════════════╝

# ── Configuration (EDIT THESE) ─────────────────────────────
DJVER="snapshot-20260416"
DOMSERVER_URL="http://10.12.13.20/domjudge/api/"
JUDGEHOST_USER="judgehost"
JUDGEHOST_PASS="passw0rd"
INSTALL_PREFIX="/opt/domjudge"
# CPU cores to use for judging (space-separated), e.g. "0 1 2 3"
JUDGE_CORES="0"
# ───────────────────────────────────────────────────────────

echo "Current hostname: $(hostname)"
read -rp "Enter new hostname (leave empty to keep current): " NEW_HOSTNAME < /dev/tty || true
if [ -n "${NEW_HOSTNAME:-}" ]; then
    sudo hostnamectl set-hostname "$NEW_HOSTNAME"
    echo "Hostname changed to: $NEW_HOSTNAME"
fi

echo "[1/8] Switching to BFSU mirror and installing dependencies..."
CODENAME=$(lsb_release -cs)
sudo rm -f /etc/apt/sources.list.d/* /etc/apt/sources.list
sudo tee /etc/apt/sources.list.d/ubuntu.sources > /dev/null <<EOF
Types: deb
URIs: https://mirrors.bfsu.edu.cn/ubuntu
Suites: ${CODENAME} ${CODENAME}-updates ${CODENAME}-backports
Components: main restricted universe multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg

Types: deb
URIs: https://mirrors.bfsu.edu.cn/ubuntu
Suites: ${CODENAME}-security
Components: main restricted universe multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
EOF
sudo apt-get update -qq
sudo apt-get install -y -qq make pkg-config sudo debootstrap libcgroup-dev \
    php-cli php-curl php-json php-xml php-zip lsof procps gcc g++ wget
sudo apt-get remove -y apport 2>/dev/null || true

echo "[2/8] Configuring kernel boot parameters (cgroup)..."
sudo sed -i 's/^GRUB_CMDLINE_LINUX_DEFAULT=.*/GRUB_CMDLINE_LINUX_DEFAULT="quiet cgroup_enable=memory swapaccount=1"/' /etc/default/grub
sudo update-grub

echo "[3/8] Downloading and building DOMjudge ${DJVER}..."
cd /tmp
if [ ! -d "domjudge-${DJVER}" ]; then
    wget --no-check-certificate -q "https://10.12.13.166:2333/static/domjudge-${DJVER}.tar.gz"
    tar xzf "domjudge-${DJVER}.tar.gz"
fi
cd "domjudge-${DJVER}"
./configure --prefix="${INSTALL_PREFIX}" --quiet
make judgehost -j"$(nproc)"
sudo make install-judgehost

echo "[4/8] Creating users and groups..."
sudo groupadd -f domjudge-run
for core in $JUDGE_CORES; do
    id "domjudge-run-${core}" &>/dev/null || \
        sudo useradd -d /nonexistent -g domjudge-run -M -s /bin/false "domjudge-run-${core}"
done

echo "[5/8] Installing sudoers rules..."
sudo cp "${INSTALL_PREFIX}/judgehost/etc/sudoers-domjudge" /etc/sudoers.d/
sudo chmod 440 /etc/sudoers.d/sudoers-domjudge

echo "[6/8] Installing systemd services..."
sudo cp judge/domjudge-judgedaemon@.service /etc/systemd/system/
sudo cp judge/create-cgroups.service /etc/systemd/system/
sudo tee -a /etc/systemd/system/domjudge-judgedaemon@.service > /dev/null <<'EOF'

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable create-cgroups --now

echo "[7/8] Building chroot environment (this may take a few minutes)..."
sudo DEBMIRROR="https://mirrors.bfsu.edu.cn/ubuntu" "${INSTALL_PREFIX}/judgehost/bin/dj_make_chroot"

echo "[8/8] Configuring REST API credentials and enabling services..."
sudo tee "${INSTALL_PREFIX}/judgehost/etc/restapi.secret" > /dev/null <<EOF
# id  URL  username  password
default ${DOMSERVER_URL} ${JUDGEHOST_USER} ${JUDGEHOST_PASS}
EOF
sudo chmod 640 "${INSTALL_PREFIX}/judgehost/etc/restapi.secret"

for core in $JUDGE_CORES; do
    sudo systemctl enable "domjudge-judgedaemon@${core}"
done

echo ""
echo "=========================================="
echo "  Judgehost setup complete!"
echo "  DOMserver: ${DOMSERVER_URL}"
echo "  Judge cores: ${JUDGE_CORES}"
echo "  NOTE: Reboot required for cgroup changes"
echo "=========================================="