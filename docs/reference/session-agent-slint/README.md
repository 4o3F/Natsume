# Phase 6 Session Agent Slint reference

This directory preserves the accepted v2.7 Slint/XDG implementation scaffold without adding Phase 6 dependencies to the current Phase 0 Cargo graph.

When Phase 6 begins, move the reviewed pieces into `client/session-agent`, admit the pinned Slint dependency through the normal dependency/lockfile process, and prove the GNOME/Wayland plus LightDM-launched X11 matrix before enabling it in production packaging.

The production contract is already frozen: direct XDG Autostart, one hidden resident process, no systemd user unit, lazy windows only after typed Daemon snapshots, Slint winit backend and Skia renderer.
