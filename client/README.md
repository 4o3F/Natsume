# Client

The signed Client package contains exactly three Natsume processes plus Caddy:

- `device-daemon`: integrated Machine-ID startup check, encrypted vault, Device-only Enrollment, mandatory-mTLS QUIC, explicit Command journal, `SYNC_STATE`-bound Gateway key/CSR/certificate lifecycle and Caddy adapter;
- `privileged-helper`: root/no-network hardware, logind and Home transactions;
- `session-agent`: XDG Autostart direct-launch process that validates its current logind graphical session; Phase 0 proves the minimal hidden/lazy Slint boundary, while Phase 6 owns the production Wayland/X11 binding UI and managed Browser behavior;
- Caddy: separate non-root loopback HTTPS data plane.

There is no Identity Guard service, installation instance, generic certificate-install Command or Session-to-Caddy lock coupling. Gateway private material is generated locally only when an explicit state sync requires it.

## Session Agent process ownership

The authenticated desktop starts `/usr/bin/natsume-session-agent --autostart` directly through XDG Autostart. The current package establishes the resident-and-hidden process boundary and has no Session Agent systemd user service or bootstrap/runtime descriptor. P0.7 must validate a minimal typed-trigger lazy Slint UI before G0; Display Manager lifecycle remains owned by Daemon/Privileged Helper/logind, and the complete production GUI remains Phase 6 work.
