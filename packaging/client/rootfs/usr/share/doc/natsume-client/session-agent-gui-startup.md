# Runbook: XDG-Autostart Session Agent and Binding UI

## Expected chain

```text
display manager authenticates the contest user
→ selected desktop session starts
→ desktop processes /etc/xdg/autostart/org.natsume.SessionAgent.desktop
→ /usr/bin/natsume-session-agent --autostart
→ Agent acquires $XDG_RUNTIME_DIR/natsume/session-agent.lock
→ Agent declares its current session ID and boot ID to org.natsume.Device1
→ Device Daemon resolves the D-Bus caller PID and verifies that exact session through logind
→ Agent registers, renews its lease and polls the current typed UI snapshot
→ Slint shows only the state selected by the Device Daemon
```

GDM and LightDM are not Session Agent IPC peers. They authenticate and start the selected desktop. The Device Daemon, not the Agent, is responsible for binding the D-Bus caller to the declared exact graphical session.

## UI states

- `Hidden`: no window is shown.
- `BindingPrompt`: the current `negotiation_id` and `submission_epoch` allow seat-code submission. The mandatory prompt cannot be closed.
- `BindingPending`: the submitted Binding input is awaiting the next Daemon snapshot; seat-code input is unavailable.

If the Device1 connection, registration, lease renewal, snapshot read or submission fails, the Agent immediately applies `Hidden`, then retries the Device1 connection. A previous Binding prompt must never remain visible after disconnect.

## Healthy evidence

- the Agent's D-Bus caller PID resolves through logind to the exact session declared at registration;
- the resolved session is the single local, active contest-user graphical session on the deployment seat;
- the system XDG entry exists once, uses absolute `--autostart`, and has no `OnlyShowIn`/`NotShowIn`;
- no `natsume-session-agent.service` user unit is installed;
- the exact user shadow path `~/.config/autostart/org.natsume.SessionAgent.desktop` is absent;
- one owner-only exclusive singleton lock exists at `$XDG_RUNTIME_DIR/natsume/session-agent.lock`;
- Device1 has one current lease for that exact session;
- the Agent continues lease renewal and snapshot polling while its Slint event loop is running;
- the visible screen matches `BindingPrompt` or `BindingPending`; `Hidden` has no visible window.

## Diagnosis order

1. From the Device Daemon, resolve the Device1 caller PID and its logind session. Confirm it exactly matches the session and boot ID declared by the Agent.
2. Verify that the desktop implements XDG Autostart and launched the package entry. Do not inspect LightDM/GDM private APIs from the Agent.
3. Check the exact user-level shadow path only; never delete unrelated autostart entries.
4. Verify the singleton owner/path and that the Agent was invoked only with `--autostart`.
5. Check Device1 registration and lease renewal. A rejected registration must be diagnosed at the Daemon caller/session check.
6. Check the latest Device1 snapshot. A disconnect must immediately hide any existing prompt before reconnecting.

## Recovery

- Wrong or ambiguous session: terminate extra sessions and create one fresh managed session.
- User-level shadow: run fixed-path managed-Home repair, then recreate the desktop session.
- Missing Agent lease or crash: terminate and recreate the managed desktop session so XDG Autostart starts a new process.
- Slint startup failure: verify target OS libraries and package dependency closure; do not fall back to external GUI helpers.

## Security checks

- No password, key, LKG or Caddy runtime JSON in Agent logs, D-Bus snapshots, argv or Slint properties.
- No Session Agent systemd user unit, `systemd-run --user`, `systemctl --user`, `dbus-update-activation-environment`, environment descriptor or display-guessing path.
- No Qt backend, runtime Slint interpreter, live preview, system tray, MCP/system-testing, WebView, Node, Python, JVM, `zenity`, `kdialog`, `yad` or `xdg-open`.
- Agent cannot call Caddy Admin or privileged hardware/Home methods.
