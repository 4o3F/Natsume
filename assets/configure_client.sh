#!/bin/sh

NATSUME_SERVER="https://localhost"
NTP_SERVER="localhost"
USER_PASSWD="passwd"


if [ "$(whoami)" = "root" ]; then
	echo "Is root user, procedding."
else
	echo "Not root user"
fi

echo "Remove sudo password for user icpc"
echo "icpc ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/icpc
chmod 440 /etc/sudoers.d/icpc


echo "NTP=$NTP_SERVER" >> /etc/systemd/timesyncd.conf
timedatectl set-timezone "Asia/Shanghai"
systemctl restart systemd-timesyncd.service

echo "Download public key into .ssh"
curl -k "$NATSUME_SERVER/static/key.pub" -o /root/.ssh/authorized_keys
curl -k "$NATSUME_SERVER/static/caddy.deb" -o /root/caddy.deb
apt install -y /root/caddy.deb

echo "Disabling Natsume service"
sudo systemctl stop "natsume"

echo "Download natsume client"
curl -k "$NATSUME_SERVER/static/natsume_client" -o /usr/bin/natsume_client
mkdir /etc/natsume
curl -k "$NATSUME_SERVER/static/client_config.toml" -o /etc/natsume/config.toml

echo "Configuring permission... IMPORTTANT!"
chown root /etc/natsume/config.toml
chmod 4701 /usr/bin/natsume_client
chmod 600 /etc/natsume/config.toml
chmod 600 /etc/caddy/Caddyfile

echo "Disabling SSH password login"
sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config && systemctl restart sshd

echo "Activating CLion"
curl -k "$NATSUME_SERVER/static/clion.key" -o /etc/skel/.config/JetBrains/CLion2022.3/config/clion.key

echo "Add new user"
if id "stu" &>/dev/null; then
    echo "User 'stu' exists. Deleting..."
    sudo userdel -r stu
    echo "User 'stu' deleted with home directory."
else
    echo "User 'stu' does not exist."
fi

useradd -m stu
echo "stu:$USER_PASSWD" | sudo chpasswd

echo "Add Natsume monitor service"

cat <<EOF | sudo tee "/etc/systemd/system/natsume.service" > /dev/null
[Unit]
Description=Natsume monitor
After=network.target network-online.target
Requires=network-online.target

[Service]
User=root
ExecStart=/usr/bin/natsume_client monitor
TimeoutStopSec=5s

[Install]
WantedBy=multi-user.target
EOF

sudo chmod 644 "/etc/systemd/system/natsume.service"
sudo systemctl daemon-reload
sudo systemctl enable "natsume"
sudo systemctl start "natsume"
sudo systemctl status "natsume"