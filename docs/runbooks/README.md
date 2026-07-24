# Runbooks

Required operational runbooks:

- single-lifetime Server reset between contests;
- one authoritative CSV import/re-import;
- Server control CSR/offline-sign/import provisioning;
- Device-only manual/automatic Enrollment and first mandatory-mTLS connection;
- Gateway certificate issuance inside authenticated `SYNC_STATE`;
- Machine ID mismatch, local vault corruption and local factory reset;
- explicit `SYNC_STATE` and human-only `SYNC_SECRET`;
- Device replacement by unbind/delete/re-enroll/rebind;
- Session Agent XDG/logind startup, GNOME/LightDM GUI diagnostics and focus-denied behavior;
- Session lock/unlock recovery and stale-epoch rejection;
- Caddy visual BLOCKED page and replay recovery;
- Home recovery;
- backup/restore and package rollback;
- fleet readiness and full contest rehearsal.

- [`session-agent-gui-startup.md`](session-agent-gui-startup.md): XDG Autostart, hidden resident Agent, logind validation and lazy Slint UI recovery.
