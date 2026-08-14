# Phase 0 target-VM package lifecycle

This harness is intentionally excluded from shared-runner CI. It mutates package state and must run only on a disposable VM whose OS/systemd baseline is frozen in [the supported platform](../../docs/supported-platform.md) and recorded with the run evidence in [Phase 0 status](../../docs/gates/phase-0-status.md).

## Inputs

- current `natsume-client` Deb;
- optional previous-version Deb for the upgrade path;
- a non-secret Server IP literal and port;
- explicit destructive-test acknowledgement.

The committed `integration-tests/fixtures/client.preseed` is documentation and a parser fixture. Override its documentation-range endpoint with the target lab endpoint through the environment.

## Run

```bash
sudo env \
  NATSUME_TARGET_VM_ACK=phase0-destructive-package-lifecycle \
  NATSUME_TEST_SERVER_IP=192.0.2.10 \
  NATSUME_TEST_SERVER_PORT=8443 \
  NATSUME_TEST_RECONFIGURE_IP=2001:db8::10 \
  NATSUME_TEST_RECONFIGURE_PORT=9443 \
  packaging/target-vm/phase0-lifecycle.sh pre-reboot \
  /absolute/path/natsume-client_current_amd64.deb \
  /absolute/path/natsume-client_previous_amd64.deb

sudo reboot

sudo env \
  NATSUME_TARGET_VM_ACK=phase0-destructive-package-lifecycle \
  packaging/target-vm/phase0-lifecycle.sh post-reboot
```

Omit the previous Deb only when testing a first-version package. The harness verifies canonical endpoint persistence across reinstall/upgrade, explicit reconfiguration, reboot persistence, remove retention and purge cleanup. A shared runner, WSL instance or non-systemd container is not G0 evidence.
