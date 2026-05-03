# Natsume

Natsume is the contest workstation orchestration tool used around DOMjudge. It contains:

- `natsume_server`: serves the sync API, panel assets, static deployment assets, and player credential database.
- `natsume_client`: binds a contest machine to a seat ID, syncs credentials into Caddy, cleans the player account, manages the graphical session, and reports sync status.
- `assets/`: deployment scripts and printing helpers for contest machines, judgehosts, and print stations.
- `data_preprocess/`: helpers that convert team XLSX sheets into DOMjudge import files and upload organization logos.

## Build

Build the binaries with the feature that matches the target role:

```bash
cargo build --release --features server --bin natsume_server
cargo build --release --features client --bin natsume_client
```

## Server workflow

1. Prepare `config.toml`, TLS CA material, and the static folder.
2. Load player credentials into the database:

   ```bash
   natsume_server -c config.toml load -d data.csv
   ```

   `data.csv` must contain `id,username,password`.

3. Start the server:

   ```bash
   natsume_server -c config.toml serve
   ```

The static folder should include the files consumed by `assets/configure_client.sh`:

- `caddy.deb`
- `yad.deb`
- `natsume_client`
- `client_config.toml`
- `cert/reverse.crt`
- `cert/reverse.key`
- `ca.crt`
- `key.pub`
- `clion.key`

## Client workflow

Install the client binary as SUID root so it can update protected contest-machine files:

```bash
chown root /usr/bin/natsume_client
chmod 4701 /usr/bin/natsume_client
chmod 600 /etc/natsume/config.toml
chmod 600 /etc/caddy/Caddyfile
```

Typical contest flow:

```bash
natsume_server -c config.toml load -d data.csv
natsume_server -c config.toml serve
natsume_client clean
natsume_client bind --id <ID>
natsume_client sync
natsume_client session auto-login
```

Available client commands:

- `bind --id <ID>` binds the machine to a contest ID.
- `bind --prompt` asks for the ID through the GUI prompt (works well for massive contests, can dispatch this task to other stuff).
- `sync` fetches the bound username/password, writes the Caddy reverse-proxy config, and reloads Caddy.
- `clean` recreates the player user and unmounts VS Code extension bind mounts before deletion.
- `session terminate` terminates the active player graphical session.
- `session auto-login` starts the player session through the current LightDM-based flow.
- `monitor` runs continuously from the systemd service and reports sync status to the server.

## Client setup script

`assets/configure_client.sh` automates a contest workstation setup. Before running it, edit the variables at the top of the script:

- `CERT_DOMAIN`
- `CERT_IP`
- `NATSUME_SERVER`
- `NTP_SERVER`
- `USER_PASSWD`
- `ROOT_PASSWD`

The script configures hosts and time sync, installs Caddy/YAD, sets Caddy admin to `localhost:20190`, downloads the Natsume client config and TLS files, disables password SSH login, tightens Firefox contest policies, recreates the `stu` player account, disables `container-vscgallery.service`, and installs `natsume.service` for `natsume_client monitor`.

## Judgehost setup script

`assets/configure_judgehost.sh` prepares a DOMjudge judgehost. Edit these variables before running:

- `DJVER`
- `DOMSERVER_URL`
- `JUDGEHOST_USER`
- `JUDGEHOST_PASS`
- `INSTALL_PREFIX`
- `JUDGE_CORES`

The script switches APT to the BFSU mirror, installs judgehost dependencies, configures cgroup kernel parameters, builds and installs the selected DOMjudge judgehost snapshot, creates `domjudge-run-*` users, installs sudoers and systemd services, builds the chroot, writes REST API credentials, and enables the judgedaemon services. Reboot after running it so the cgroup boot parameters take effect.

## Printing helpers

`assets/print_typst.py` runs on the DOMjudge side. It accepts DOMjudge print-script arguments, validates source MIME type, renders the submission to PDF with Typst, stores a backup in `/opt/domjudge/print_backup`, and POSTs the PDF to a configured print station.

`assets/print_client.py` runs on the print station. It listens on port `12306`, receives PDF payloads, prints them with SumatraPDF, writes `Print.log`, and copies PDFs into `Success/` or `Error/`.

## Data preprocessing

See `data_preprocess/README.md` for the XLSX schema and generated DOMjudge files. The current schema is:

```text
organization,team_name_en,team_name_zh,seat,account,password,category
```

`category` is used as each generated team's `group_ids` entry, and team display names are generated as `team_name_zh(team_name_en)`.

## Security notes

- Keep `/etc/natsume/config.toml` and `/etc/caddy/Caddyfile` unreadable by normal users because they contain contest credentials or routing secrets.
- Use the self-signed CA files distributed by the server static directory for client/server TLS trust.
- Do not leave API tokens, Basic Auth credentials, or generated import files in public static directories unless they are intended for distribution.
