# Target-VM package lifecycle

This harness is intentionally excluded from shared-runner CI. It mutates package
state and must run only on a disposable VM. Record the exact OS/systemd baseline
with the run evidence required by the
[target architecture](../../docs/architecture.md#19-验证策略).

## Inputs

- current `natsume-client` Deb;
- optional previous-version Deb for the upgrade path;
- a non-secret Server IP literal and port;
- explicit destructive-test acknowledgement.

Supply the target lab endpoint through the environment; the repository does not
carry a deployment-specific debconf preseed.

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

The pre-reboot phase refuses to run when `natsume-client` is already installed or `/etc/natsume` already exists: stale residue (for example from an old V1 installation) changes dpkg's install branches and contaminates the evidence. Use a freshly provisioned VM or restore a clean snapshot.

After provisioning the contestant account and Home template on a separate disposable
VM snapshot, run the [Home mount acceptance checks](home-reset.md) for host visibility,
namespace isolation rejection and same-epoch recovery.
