# Runbook: GNOME Kiosk Session Agent

The image installs the distribution's official GNOME Kiosk and script-session
packages. GDM automatically logs `waiting` into `gnome-kiosk-script-wayland`;
`teams` (the `contest` role) uses the independent `ubuntu-wayland` session. No compositor or GDM patches
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

BindingPrompt and BindingPending use the same window. The seat-binding page uses
a warm light layout: instructions on the left and the seat input on the right.
Visible copy calls this a seat (座位), and displays readable messages for unknown,
unmapped or occupied seat codes. Input focuses automatically; Enter, keyboard
button activation and mouse confirmation use the same current intent and epoch.
Empty input cannot be submitted. Pending confirmation has a dedicated status view. Disconnection or an expired
lease removes Binding input, retains the last non-secret team/school/seat and logo,
and displays an offline badge. A cold Agent without a Daemon starts with the
placeholder. Ordinary foreground switching keeps both native sessions and the
background Agent lease alive. Native Wayland may defer frames while hidden.
Activation uses the live authenticated Agent; maintenance and foreground readiness
still wait for the current fullscreen frame after activation. A deferred background
frame alone must not consume the waiting recovery budget.

Bound waiting uses a black background with a large centered school logo and
bilingual school name. Both the placeholder and bound Waiting screen show
Project Natsume at the top left, with a separate author row showing "by", the
avatar, and 4o3F. Packaging downloads the
512 × 512 Gravatar image into the Deb at `/usr/share/natsume/author-avatar.png`;
the Agent loads it locally once at window creation, without network access.
A missing or unreadable avatar does not block Waiting or first-frame readiness.
The bottom bar places the bilingual team name on the left
and the assigned seat on the right; the offline icon and text stay at the top right. Missing one language uses the available name; missing
logos use a default icon. Long text wraps and can scroll; the seat remains visible.
The package depends on Noto CJK fonts. A missing logo never prevents first-frame
readiness or makes an online device appear offline.

The Daemon stores display-only metadata in `/var/lib/natsume/state/waiting.json`
(mode 0600) and public PNGs in `/var/lib/natsume-display` (directory 0755,
files 0644, owned by natsume). waiting can read the PNGs but cannot modify them or
read the private state directory. Home reset does not affect this cache. Its
scope includes Server endpoint, pinned Control CA, enrolled Device ID and public
control key. A valid unbind clears the durable presentation; rebinds cannot reuse
an old school's image. Do not copy the private state directory between machines.

Startup restores matching cached data offline. Only a successful control
handshake followed by a complete valid snapshot clears that badge. The Daemon
fetches logos over the same pinned HTTPS connection settings, with a 20-second
request timeout and a one-minute ETag refresh. It checks content and image bounds,
converts raster images to PNG, and retries failures independently of resource
reconciliation. A cache write failure is reported in the Daemon journal and
closes control; fix the directory ownership or storage failure before restarting.

Both local peers require Session Agent protocol 3 at registration; Device control
uses `natsume.control.v3`. Upgrade Server, Daemon and Agent together, and import
complete team profiles before reconnecting previously bound devices. Each frame
also carries the registration lease ID, so a pre-restart frame cannot establish
readiness after a new registration.

For diagnosis, inspect the waiting user's `org.gnome.Kiosk.Script.service` and
`org.gnome.Kiosk@wayland.service`, the corresponding journal, logind User.Display,
Device1 registration/confirmation errors, and the compositor-confirmed Wayland fullscreen state and output size.
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
Verify effective GNOME idle/power settings and actual output in the actual session,
including an idle interval longer than the old timeout. A historical frame
confirmation is not evidence that the monitor still has active output.

The compatible Client Deb provides all image configuration inputs under
`/usr/share/natsume/image-integration/`; apply its deployment manifest during
image construction or planned maintenance, after provisioning waiting/teams.
