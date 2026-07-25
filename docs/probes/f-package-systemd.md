# Probe F — Package 与 systemd

> Status: `NOT-RUN`  
> Requirements: `REQ-P0-003`、`REQ-P0-014`、`REQ-P0-040`–`REQ-P0-046`、`REQ-P0-065`  
> Gates: `G0-002`、`G0-010`、`G0-012`、`G0-013`

## 目标

证明 Server/Client Deb 的内容、权限、服务拓扑、preseed、lifecycle、reboot 和禁止项。

## 环境

```text
COMMIT_SHA=
SERVER_OS=
CLIENT_OS=
NFPM_VERSION=
SERVER_PACKAGE=
CLIENT_PACKAGE=
OWNER=
REVIEWER=
DATE=
```

## Lifecycle

分别验证：

- clean install；
- preseed；
- interactive configure；
- reinstall；
- upgrade；
- explicit reconfigure；
- reboot；
- service restart；
- remove；
- purge；
- rollback。

## Manifest/policy

检查：

- users/groups；
- directories/modes；
- sysusers/tmpfiles；
- Server/Device Daemon/Helper services；
- D-Bus policy；
- Caddy binary/config；
- XDG Autostart；
- endpoint preservation；
- package-owned static assets。

必须不存在：

- Session Agent systemd user unit；
- Identity Guard；
- `LoadCredential`/`SetCredential` product secret；
- postinstall/runtime download；
- token/CA/private key generation；
- bundled private key/secret；
- duplicate staging/build；
- external GUI runtime/helper。

## 结果

```text
STATUS=NOT-RUN
ARTIFACTS=
LIMITATIONS=
```
