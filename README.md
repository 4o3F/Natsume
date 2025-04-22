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
9. TODO

### Server
1. Use `natsume_server -c config.toml serve` to init database and check config
2. Use `natsume_server -c config.toml load -d data.csv` to load player info into database
3. TODO

## Process

**Client**
+ Use `bind` command to bind the MAC address with seat ID.
+ Use `lock` command to lock all user out.
+ Use `unlock` command to log user in.
+ Use `clean` command to clear user data.
+ Use `sync` command to fetch contest username and password from server according to ID.

**Server**
+ Use `serve` command to start the server.
+ Use `load` command to load contest username and password to server database.

The total process should be as follow.  
`load` -> `serve` -> `clean` -> `bind` -> `sync` -> `unlock`

## Tech details
+ The client binary should be set SUID permission so that `bind` command can write to the Caddyfile to set username and password.
+ The Caddyfile need to be set as not readble by other users, to prevent password leak.
+ Username and password fetch need to be encrypted using AES, to make sure others won't know what's inside.


## Retrive MAC from IP
```
ip -o -4 addr show | awk 'split($4, a, "/") && a[1]==ip {print $2}' ip="目标IP" | xargs -I{} ip link show {} | awk '/link\/ether/ {print $2}'
```

Direct retrive via route
```
ip route get 目标IP | awk '{for(i=1;i<=NF;i++) if($i=="dev") print $(i+1)}' | xargs -r -I{} ip -o link show {} | awk '{for(i=1;i<=NF;i++) if($i=="link/ether") print $(i+1)}'
```
