# Client

The signed Client package contains exactly three Natsume processes plus Caddy:

- `device-daemon`: integrated Machine-ID startup check, encrypted vault, provisioning-window Enrollment for a Device Token and Gateway certificate, Device Token-authenticated WSS, explicit Command journal and Caddy adapter;
- `privileged-helper`: root/no-network hardware, logind and Home transactions;
- `session-agent`: XDG Autostart direct-launch process that validates its current logind graphical session; Phase 0 proves the minimal hidden/lazy Slint boundary, while Phase 6 owns the production Wayland/X11 binding UI and managed Browser behavior;
- Caddy: separate non-root loopback HTTPS data plane.

There is no Identity Guard service, installation instance, generic certificate-install Command or Session-to-Caddy lock coupling. Gateway private material is generated locally for Enrollment and never leaves the Device.

## Session Agent process ownership

The agent exits 1 with the stable stderr id `NATSUME_SESSION_AGENT_LOGGING_INIT_FAILED` when logging cannot initialize, 2 on an invalid invocation, and 3 when the Slint event loop cannot start or fails.

The authenticated desktop starts `/usr/bin/natsume-session-agent --autostart` directly through XDG Autostart. The current package establishes the resident-and-hidden process boundary and has no Session Agent systemd user service or bootstrap/runtime descriptor. The minimal typed-trigger lazy Slint UI probe is desktop-capability evidence: its acceptance criteria are frozen in [the supported platform](../docs/supported-platform.md) and its status is tracked in [Phase 0 status](../docs/gates/phase-0-status.md); Display Manager lifecycle remains owned by Daemon/Privileged Helper/logind, and the complete production GUI remains Phase 6 work.
