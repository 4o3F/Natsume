# ADR-0017: Cross-desktop Session Agent bootstrap and hand-built native GUI

- Status: Superseded
- Date: 2026-07-23
- Superseded by: ADR-0018

ADR-0017 introduced a portable XDG entry but then split the Agent into bootstrap/runtime modes, added a static systemd user unit and assembled a GUI directly from winit, softbuffer, tiny-skia and cosmic-text.

The retained conclusions are that the Agent must run in the authenticated desktop session, use logind to reject greeter/remote/ineligible sessions, preserve typed secret-free UI payloads, treat Wayland focus denial as observable rather than fatal, and remain independent of Display Manager private APIs.

The startup handoff, user-unit supervision and hand-built GUI stack are superseded by ADR-0018.
