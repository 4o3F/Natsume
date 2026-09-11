# Runbook: GNOME Kiosk Session Agent

The image installs the distribution's official GNOME Kiosk and script-session
packages. GDM automatically logs `waiting` into `gnome-kiosk-script-xorg`;
`teams` (the `contest` role) uses the independent `ubuntu-xorg` session. No compositor or GDM patches
are used.

The Client package supplies a drop-in for `org.gnome.Kiosk.Script.service`:
it runs `/usr/bin/natsume-session-agent run` only as `waiting`, with a one-second
restart delay and at most five starts within sixty seconds. The official Kiosk
session starts and stops this service. There is no second XDG Autostart entry,
Agent RegisterClient restart owner, shell keeper, or user-home launch script.

The Agent first displays its local black fullscreen Slint/Skia window, optionally
using `/usr/share/natsume/waiting.png`. Device1 and Server availability are not
prerequisites for displaying this placeholder. It retains its singleton lock at
`$XDG_RUNTIME_DIR/natsume/session-agent.lock`.

GNOME's user manager may omit `XDG_SESSION_ID`. The Agent then resolves its own
user's Display through logind before registering. Device1 independently checks
the actual bus caller UID/PID, exact boot/session and the strict user-manager
mapping. Neither an environment variable nor registration proves readiness:
the exact current lease must confirm a rendered fullscreen frame at the real
monitor size, and Helper must independently observe the running kiosk desktop.

BindingPrompt and BindingPending use the same window. Disconnection or an expired
lease removes Binding input and restores the placeholder. Ordinary foreground
switching keeps both native sessions and the background Agent lease alive.

For diagnosis, inspect the waiting user's `org.gnome.Kiosk.Script.service` and
`org.gnome.Kiosk@x11.service`, the corresponding journal, logind User.Display,
Device1 registration/confirmation errors, and the actual X11 window geometry.
Do not repair an identity rejection by inventing a session ID or relaxing caller
checks. Do not start a second Agent manually alongside the Kiosk service.

The service handles application crashes first. After at least sixty seconds of
observed sustained failure, Helper can rebuild waiting once per boot, preserving
contest and GDM. Its root runtime record retains the captured session and spent
budget across service restarts; never delete that record to force another attempt.
Helper resumes an interrupted capture before accepting a new recovery. A second
sustained failure keeps the display error and requires administrator maintenance
or a normal reboot. Full package/image acceptance remains required before release.

GNOME Kiosk 46 selects the `gnomekiosk` dconf profile itself. The image must install
its policy in `/etc/dconf/profile/gnomekiosk`, retaining the distribution file-db;
setting only `DCONF_PROFILE=natsume_waiting` does not configure this compositor.
Verify effective settings and X11 screen-saver/DPMS state in the actual session,
including an idle interval longer than the old timeout. A historical frame
confirmation is not evidence that the monitor still has active output.

The compatible Client Deb provides all image configuration inputs under
`/usr/share/natsume/image-integration/`; apply its deployment manifest during
image construction or planned maintenance, after provisioning waiting/teams.
