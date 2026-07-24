# ADR-0018: XDG-Autostart direct resident Session Agent with lazy Slint UI

- Status: Accepted
- Date: 2026-07-23
- Supersedes: ADR-0017 startup/supervision/GUI implementation

## Context

Session Agent must execute inside the authenticated desktop session so it receives the real Wayland/X11 socket, session bus, locale, fonts and IME environment. GDM and LightDM are Display Managers below that session; they are not Agent UI backends or Agent IPC peers. A second systemd-user launch plane adds environment handoff, restart and ownership states without improving the product security boundary. Building widgets, shaping, input and rendering directly from low-level crates also duplicates mature GUI-library work.

## Decision

1. Install one system-wide XDG Autostart entry with absolute `Exec=/usr/bin/natsume-session-agent --autostart` and no desktop-specific scope.
2. The same process validates its own PID through logind, acquires an owner-only singleton below `XDG_RUNTIME_DIR`, registers with the Daemon and renews a lease.
3. Do not install a Session Agent systemd user unit. Do not implement bootstrap/run modes, an environment descriptor, `systemd-run --user`, display guessing or Daemon-spawned GUI recovery.
4. The normal state is `ready + hidden`: the process runs the Slint event loop but creates no top-level component until a typed UI snapshot requests presentation.
5. Compile package-owned `.slint` files at build time with `slint-build`. Production explicitly enables Slint `backend-winit`, `renderer-skia`, `std`, `compat-1-2` and accessibility, while disabling Qt, interpreter, live-preview, tray, MCP and system-testing features.
6. Natsume product code no longer directly implements winit/softbuffer/tiny-skia/cosmic-text GUI plumbing.
7. The zbus/Tokio worker delivers UI changes through `slint::invoke_from_event_loop`; all Slint component access remains on the event-loop thread.
8. XDG Autostart is not a crash supervisor. Agent loss expires the lease, gates the Browser and requires a fresh managed desktop session to rerun Autostart. The Daemon does not synthesize display state.
9. Runtime session lifecycle intent belongs to Daemon → Privileged Helper/logind. Agent does not call LightDM/GDM private interfaces.

## Consequences

- One startup path and one process own the graphical-session state.
- There is no automatic in-session restart after Agent crash; fail-closed lease behavior and managed session replacement are operationally explicit.
- Slint/Skia add build size and native dependency work, which becomes a package/ELF/reproducibility Gate.
- GNOME/GDM/Wayland and a target LightDM-started X11 desktop must pass real login, hidden steady-state, lazy-window, IME, focus-denied and crash/relogin tests.
- Protocol/database contracts remove `SessionSupervisor`; field number 4 remains reserved in Protobuf.
