# Runbook: XDG-Autostart Session Agent and lazy Slint UI

## Expected chain

```text
display manager authenticates the contest user
→ selected desktop session starts
→ desktop processes /etc/xdg/autostart/org.natsume.SessionAgent.desktop
→ /usr/bin/natsume-session-agent --autostart
→ Agent validates its own PID/session through logind
→ Agent acquires $XDG_RUNTIME_DIR/natsume/session-agent.lock
→ Agent registers with org.natsume.Device1 and renews its lease
→ Agent enters the Slint event loop with no visible window
→ a typed UI snapshot lazily creates/shows the Slint component
```

GDM and LightDM are not Session Agent IPC peers. They authenticate and start the selected desktop. Runtime session intent belongs to the Device Daemon and Privileged Helper/logind boundary.

## Healthy evidence

- exactly one local, active contest-user graphical logind session;
- session type is `wayland` or `x11`, class is `user`, remote is false, and seat matches deployment;
- the system XDG entry exists once, uses absolute `--autostart`, and has no `OnlyShowIn`/`NotShowIn`;
- no `natsume-session-agent.service` user unit is installed;
- the exact user shadow path `~/.config/autostart/org.natsume.SessionAgent.desktop` is absent;
- one owner-only singleton exists below the current `XDG_RUNTIME_DIR`;
- Agent registration reports the expected display backend and a current lease;
- the normal steady state is `ready + hidden`: no window, tray or splash;
- a Binding snapshot results in focused or explicitly unfocused presentation, never silent absence.

## Diagnosis order

1. Resolve the Agent PID through logind. Reject greeter, SSH, TTY, remote, inactive, wrong-UID/seat or ambiguous sessions.
2. Verify that the desktop implements XDG Autostart and launched the package entry. Do not inspect LightDM/GDM private APIs from the Agent.
3. Check the exact user-level shadow path only; never delete unrelated autostart entries.
4. Verify the singleton owner/path and that the Agent was invoked only with `--autostart`.
5. Check Agent registration, lease, display backend and `ready + hidden` observation.
6. On a typed UI trigger, check Slint presentation state. `presented_unfocused` is valid under Wayland; do not add focus-stealing loops.
7. If the Agent crashed or the display connection was lost, keep the Browser gated and create a fresh managed desktop session. The Daemon must not fabricate `DISPLAY`/`WAYLAND_DISPLAY` or spawn the GUI.

## Stable conditions

- `SESSION_INELIGIBLE`: wrong UID/class/type/active/remote/seat.
- `SESSION_AMBIGUOUS`: more than one eligible graphical session.
- `SESSION_AGENT_DUPLICATE`: singleton already owned by the current session.
- `SESSION_AUTOSTART_SHADOWED`: same-name user override exists.
- `SESSION_AGENT_MISSING`: an eligible session exists but no valid lease arrived.
- `SESSION_DISPLAY_UNAVAILABLE`: Slint cannot open the inherited Wayland/X11 connection.
- `SESSION_DISPLAY_LOST`: an established display connection ended.
- `SESSION_UI_PRESENTED_UNFOCUSED`: window is mapped but focus was denied.
- `SESSION_LOCK_UNSUPPORTED` / `SESSION_UNLOCK_UNSUPPORTED`: desktop lock state cannot be safely requested or verified.

## Recovery

- Wrong/ambiguous session: terminate extra sessions and create one fresh managed session.
- User-level shadow: run fixed-path managed-Home repair, then recreate the desktop session.
- Missing Agent lease or crash: keep Browser gated; terminate/recreate the managed desktop session so XDG Autostart starts a new process.
- Display loss: stop the managed Browser, expire registration and recreate the desktop session.
- Focus denied: use the standard notification or manual activation; this is not a security failure.
- Slint backend failure: verify target OS libraries, fonts/IME and package dependency closure; do not fall back to external GUI helpers.

## Security checks

- No password, key, LKG or Caddy runtime JSON in Agent logs, D-Bus snapshots, argv or Slint properties.
- No Session Agent systemd user unit, `systemd-run --user`, `systemctl --user`, `dbus-update-activation-environment`, environment descriptor or display-guessing path.
- No Qt backend, runtime Slint interpreter, live preview, system tray, MCP/system-testing, WebView, Node, Python, JVM, `zenity`, `kdialog`, `yad` or `xdg-open`.
- Agent cannot call Caddy Admin or privileged hardware/Home methods.
