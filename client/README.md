# Client

The signed Client package contains exactly three Natsume processes plus Caddy:

- `device-daemon`: integrated Machine-ID startup check, encrypted vault, Device-only Enrollment, mandatory-mTLS QUIC, explicit Command journal, `SYNC_STATE`-bound Gateway key/CSR/certificate lifecycle and Caddy adapter;
- `privileged-helper`: root/no-network hardware, logind and Home transactions;
- `session-agent`: XDG/logind graphical-session bootstrap, native Wayland/X11 binding UI, desktop-only lock presentation and managed Browser launch;
- Caddy: separate non-root loopback HTTPS data plane.

There is no Identity Guard service, installation instance, generic certificate-install Command or Session-to-Caddy lock coupling. Gateway private material is generated locally only when an explicit state sync requires it.

## Session Agent process ownership

The authenticated desktop starts `/usr/bin/natsume-session-agent --autostart` directly through XDG Autostart. The Agent starts hidden and uses Slint only after a typed UI trigger. Display Manager lifecycle work belongs to Daemon/Privileged Helper/logind; Agent has no LightDM/GDM private integration and no systemd user service.
