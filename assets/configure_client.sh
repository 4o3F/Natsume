#!/bin/sh

CERT_DOMAIN="tester.icpc"
CERT_IP="127.0.0.1"
NATSUME_SERVER="https://localhost"
NTP_SERVER="localhost"
USER_PASSWD="passwd"


if [ "$(whoami)" = "root" ]; then
	echo "Is root user, procedding."
else
	echo "Not root user"
fi

echo "Configuring hosts entry for certificate domain"
tmp_hosts="$(mktemp)"
awk -v domain="$CERT_DOMAIN" '
{
    has_domain=0
    for (i = 2; i <= NF; i++) {
        if ($i == domain) {
            has_domain=1
            break
        }
    }
    if (!has_domain) {
        print
    }
}
' /etc/hosts > "$tmp_hosts"
cat "$tmp_hosts" > /etc/hosts
rm -f "$tmp_hosts"
echo "$CERT_IP $CERT_DOMAIN" >> /etc/hosts

echo "Remove sudo password for user icpc"
echo "icpc ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/icpc
chmod 440 /etc/sudoers.d/icpc


echo "NTP=$NTP_SERVER" >> /etc/systemd/timesyncd.conf
timedatectl set-timezone "Asia/Shanghai"
systemctl restart systemd-timesyncd.service

echo "Download public key into .ssh"
curl -s -k "$NATSUME_SERVER/static/key.pub" -o /root/.ssh/authorized_keys
curl -s -k "$NATSUME_SERVER/static/caddy.deb" -o /root/caddy.deb
curl -s -k "$NATSUME_SERVER/static/yad.deb" -o /root/yad.deb
dpkg -i /root/caddy.deb
dpkg -i /root/yad.deb

echo "Disabling Natsume service"
sudo systemctl stop "natsume"

echo "Disabling Nginx service"
sudo systemctl stop "nginx"
sudo systemctl disable "nginx"

sudo systemctl stop container-vscgallery.service
sudo systemctl disable container-vscgallery.service

echo "Download natsume client"
curl -s -k "$NATSUME_SERVER/static/natsume_client" -o /usr/bin/natsume_client
mkdir /etc/natsume
mkdir /etc/natsume/cert
curl -s -k "$NATSUME_SERVER/static/client_config.toml" -o /etc/natsume/config.toml
curl -s -k "$NATSUME_SERVER/static/cert/reverse.crt" -o /etc/natsume/cert/reverse.crt
curl -s -k "$NATSUME_SERVER/static/cert/reverse.key" -o /etc/natsume/cert/reverse.key
curl -s -k "$NATSUME_SERVER/static/ca.crt" -o /etc/natsume/ca.crt

echo "Configuring permission... IMPORTTANT!"
chown root /etc/natsume/config.toml
chmod 4701 /usr/bin/natsume_client
chmod 600 /etc/natsume/config.toml
chown caddy:caddy /etc/caddy/Caddyfile
chmod 600 /etc/caddy/Caddyfile

echo "Disabling SSH password login"
sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config && systemctl restart sshd

echo "Activating CLion"
curl -s -k "$NATSUME_SERVER/static/clion.key" -o /etc/skel/.config/JetBrains/CLion2025.2/clion.key

echo "Configure Firefox"
mkdir -p /etc/firefox/policies
cat << EOF > /etc/firefox/policies/policies.json
{
  "policies": {
    "Homepage": {
      "URL": "https://$CERT_DOMAIN/",
      "Locked": true,
      "StartPage": "homepage"
    },

    "Permissions": {
      "Notifications": {
        "Allow": [
          "https://$CERT_DOMAIN/"
        ]
      }
    },

    "Certificates": {
      "Install": [
        "/etc/natsume/ca.crt"
      ]
    }
  }
}
EOF
chmod 644 /etc/firefox/policies/policies.json

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
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

sudo chmod 644 "/etc/systemd/system/natsume.service"
sudo systemctl daemon-reload
sudo systemctl enable "natsume"
sudo systemctl start "natsume"
# sudo systemctl status "natsume"
