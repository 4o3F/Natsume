# Session/Home access acceptance (C02)

Use a disposable target image with the current Client package, an enrolled/bound
device, real Caddy and a test DOMjudge upstream. Keep the Daemon running. Record
the commit, package checksum, image, desktop and Caddy versions. Home reset cases
are destructive; use the same image/template prerequisites as [Home reset](home-reset.md).

Record Target and Actual epochs, the Helper journal, display-manager/logind state,
and the non-secret `/run/natsume/caddy-mode.json`. Do not archive Caddy's active
configuration or admin response: a READY configuration contains credentials.
Measure the proxy result from an administrator process that survives contestant
logout. Use the installed Origin CA with TLS verification and a benign upstream
path; avoid login requests and recording credential headers.

1. Begin with one active contestant session, Steady Home, completed target epochs,
   and READY access. Lock and unlock the session. With otherwise healthy state,
   both states must preserve access and the same Binding relation.
2. Log out, then separately produce multiple contestant graphical sessions on a
   suitable VM snapshot. After the next completed local observation, require
   None/Ambiguous Actual, hidden Binding UI, rejected new submissions and BLOCKED
   proxy behavior (503 with usable Gateway material, otherwise no listener).
   The Server connection and recovery path remain available. Repeat while a test
   Server keeps sending valid heartbeats but delays its Target response to
   ClientState; local closure must not wait for that response.
3. Start from READY and request a new terminate/reset epoch. Inject a temporary
   Helper failure or pause at the operation boundary in an unstripped test build.
   Verify BLOCKED is already loaded before the first destructive effect. Keep the
   previous completed epoch visible throughout failure/retry. The same bound
   account and credentials must remain selected after recovery.
4. Complete a Home reset, then simulate lost mount evidence while no contestant
   session is running. With the same target and completed epoch, require
   RecoveryRequired and BLOCKED after observation. Restore the managed mount
   through the existing Helper recovery path. Once a unique Active/Locked session
   and all exact epochs are verified by the current plan, access must recover.
5. Replace a pending target while its Helper operation is in flight. The old plan
   must not publish completion for the replacement or restore Binding eligibility;
   the new epoch must remain blocked until its own completion is verified. Repeat
   with a new login between Session reconciliation and Home completion: a stale
   pre-reset Active observation must never grant access after Home has stopped it.
6. Make Caddy reload/admin unavailable while READY, then request a new reset or
   terminate. Require no Helper destructive effect before confirmed BLOCKED. If
   closure cannot be confirmed, require Daemon failure and the packaged systemd
   stop hook's hard termination of Caddy; it must restart with its blocked bootstrap.

The existing observation interval is 30 seconds; in-flight bounded local work can
delay the next observation. Record detection and closure timestamps separately.
These checks concern subsequent requests through the local proxy, not general
host networking or rollback of requests already in flight.

Automated tests use a mock Caddy command/admin endpoint, real local TLS certificate
sampling, and mock Helper D-Bus calls. They verify orchestration, exact epochs,
fencing and failures; they do not replace this real desktop/proxy acceptance.
