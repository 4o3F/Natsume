# Natsume

## Checklist
### Client
1. Install caddy with deb package
2. Add sudo user to `caddy` group, and make sure `/etc/caddy/Caddyfile` has permission 660
3. Download Natsume client into `/usr/bin/`
4. Use `chown <sudoer> natsume_client` to change client owner to sudoer, then use `chmod 4701 natsume_client` to change it's permission
5. Download cient config into `/etc/natsume/config.toml`
6. Use `chown <sudoer> /etc/natsume/config.toml` to change client config owner to sudoer, then use `chmod 600 /etc/natsume/config.toml` to change it's permission
7. Use `natsume_client bind --id <ID>` to bind this device to the given ID
8. When the username and password need to refresh, use `natsume_client sync` to overwrite the Caddyfile and reload service
9. Use `natsume_client clean` to clean the player data
10. Use `natsume_client session terminate` to end the player session
11. Use `natsume_client session auto-login` to auto login to player account

### Server
1. Use `natsume_server -c config.toml serve` to init database and check config
2. Use `natsume_server -c config.toml load -d data.csv` to load player info into database, CSV format is `id,username,password`
3. The static folder should contain the following files:
   + `caddy.deb` as the caddy installation deb
   + `natsume_client`
   + `client_config.toml` as the config that need to be sent to client
   + `clion.key` as the activation file for CLion
   + `natsume.service` as the systemd service
4. TODO

## Process

**Client**
+ Use `bind` command to bind the MAC address with seat ID.
+ Use `clean` command to clear user data.
+ Use `sync` command to fetch contest username and password from server according to ID.
+ Use `session terminate` command to terminate player session
+ Use `session auto-login` command to auto login to player account

**Server**
+ Use `serve` command to start the server.
+ Use `load` command to load contest username and password to server database.

The total process should be as follow.  
`load` -> `serve` -> `clean` -> `bind` -> `sync` -> `session auto-login`

## Tech details
+ The client binary should be set SUID permission so that `bind` command can write to the Caddyfile to set username and password.
+ The Caddyfile need to be set as not readble by other users, to prevent password leak.
+ ~~Username and password fetch need to be encrypted using AES, to make sure others won't know what's inside.~~ Use self signed cert for TLS instead.

