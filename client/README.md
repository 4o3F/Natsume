# Client

The Client package contains three Natsume processes plus Caddy:

- `device-daemon`: machine identity, Enrollment and Device Control coordinator;
- `privileged-helper`: root, no-network, capability-oriented system adapter;
- `session-agent`: XDG Autostart UI process for the current graphical session;
- Caddy: separate non-root loopback HTTPS data plane.

The target responsibilities, trust boundaries and Client-local Reconciler model
are defined only by the [target architecture](../docs/architecture.md). Current
crate contents may still be a partial implementation of that target.

## Session Agent process ownership

The agent exits 1 with the stable stderr id
`NATSUME_SESSION_AGENT_LOGGING_INIT_FAILED` when logging cannot initialize, 2 on
an invalid invocation, and 3 when the Slint event loop cannot start or fails.

The authenticated desktop starts
`/usr/bin/natsume-session-agent --autostart` directly through system-wide XDG
Autostart. The package must not install a Session Agent systemd user service,
bootstrap/runtime descriptor, runtime `.slint` interpreter or external GUI
helper.
