# Photo workstation UI port

The visual reference is the local React prototype `Natsume Photo 摄影工作台`.
The executable uses native Slint; it does not embed React, a web view, JavaScript,
the prototype's simulated filesystem, or its local-storage database.

## Source mapping

| Reference | Native implementation |
| --- | --- |
| `src/index.css`, `ui/Button`, `ui/Section` | `style.slint`: colors, typography, flat actions, fields and rules |
| `screens/Shell` | `window.slint`: black navigation bar, status band, error and status messages |
| `screens/init/*` | Initialization sidebar and form; `controller/bindings.rs` calls the real initializer |
| `screens/Overview`, `screens/SessionSetup` | Local overview, real monitor enumeration and hot-folder admission |
| `screens/workbench/*` | Team / photo / confirmation columns, candidate filmstrip and confirmation dialog |
| `screens/Unassigned`, `screens/Finals`, `screens/Export` | Real review assignments, verified final preview and offline ZIP export |
| `display/DisplayView` | Independent `ContestantWindow`, containing only public team and image properties |
| `state/store` and browser display channel | `application.rs`, backend `CaptureSession`, typed commands and snapshots |

Paper `#f4f3ef`, ink `#111111`, red `#e2231a`, rule `#d6d4ce`,
green `#0b7a3e`, amber `#c77700` and error `#b00020` come directly from the
reference. The UI retains the horizontal black header, square controls, strong
heading hierarchy, bilingual team labels and 160 × 120 candidate thumbnails.
Fonts use the reference's installed Noto Sans SC and Cascadia Mono families.
System fallback and native title bars can differ between machines; this is not a
claim of pixel-identical output on every Windows configuration.

## Intentional behavior differences

- Prototype simulators, fault-injection toggles, fake teams, fake display lists,
  generated photo artwork and reset buttons are omitted.
- Opening an existing authorized workspace is offline. Initial enrollment uses
  the existing HTTPS initializer, administrator pairing code and Windows vault.
  No separate, simulated connection-check result is presented.
- Initialization is one backend operation; the UI does not fabricate intermediate
  registration/download progress or claim a station identity before completion.
- Unconfirmed groups are in memory only. No prototype recovery screen or
  local-storage recovery is copied into the product.
- Candidate selection and automatic latest-photo mode use backend presentation
  requests. A manual candidate click requests that photo's presentation; there is
  no second mock "show" operation or unsupported exclude-photo operation.
- No contestant waiting page, default photo, seat information, simulated event
  name, historical result list or server-upload success is invented.
- Single-monitor startup requires explicit confirmation. The two native windows
  remain windowed; a selected second display is fullscreen in dual-monitor mode.
- The operator checks verbal satisfaction after the exact frame was rendered.
  Frame changes clear that check. A result preview never produces a receipt.
- ZIP publication and completion-record failure are reported separately. The UI
  currently serializes manual operations during export; camera ingestion remains
  owned by its independent backend worker.

## Ownership and frame acknowledgement

The Slint thread owns the windows. An application thread owns the Tokio runtime,
workspace and capture session. The bounded command channel carries operator
requests; a watch channel carries the latest facts. Image files are opened with
read-only sharing and checked against the expected hash, length and dimensions
before decoding off the UI thread. Only display-sized pixels and thumbnail
pixels are retained by the UI.

Each native display owns its own pending rendering slot, teardown flag and
generation. A snapshot from another generation cannot install an image or arm a
receipt. Only `AfterRendering` consumes that window's pending request. The
`unstable-wgpu-30` Slint feature enables rendering notifications on the selected
Skia/WGPU path; selecting pixels or completing a read never acknowledges them.
Closing, minimizing, monitor loss and renderer teardown invalidate the display.
Confirmation also checks display availability immediately before queuing intent.

The UI makes no database queries and does not change the schema. Typed group
commands include the expected group identity; an old queued command cannot end
or abandon the next group. Normal shutdown drains accepted file/export work.

## Verification

The reference has no automated test suite. The characterization and adapter
tests are in `backend/tests/unit/ui_application.rs`:

| Native test | Evidence |
| --- | --- |
| `ui_owner_requires_exact_frame_then_saves_and_cleans_without_deleting_camera_source` | Exact receipt required; save/finish/cleanup preserve the source and result; final preview is independent |
| `application_shutdown_leaves_a_final_snapshot_after_the_sender_closes` | Final state survives Sender closure; the window loop must terminate on closure |
| `stale_group_command_cannot_abandon_the_retake_group` | Old queued group identity cannot affect a retake |
| `changed_image_never_produces_pixels_or_a_receipt` | Tampered normalized input is rejected before display |
| `native_windows_render_each_page_and_acknowledge_only_a_rendered_photo` | Real Skia windows, page screenshots, actual render notification and backend confirmation |
| `live_server_ui_initialization_through_signed_zip` | Opt-in real HTTPS Server, Windows vault, production Slint callbacks and display adapter, photo ingestion/confirmation/cleanup, signed ZIP and read-only reopen |

```powershell
cargo run -p natsume-photo-client --locked
cargo test -p natsume-photo-client -p natsume-photo-protocol --locked
cargo test -p natsume-photo-client --bin natsume-photo-client native_windows_render -- --ignored --nocapture
cargo clippy -p natsume-photo-client --all-targets --all-features -- -D warnings
cargo doc -p natsume-photo-client --no-deps
prek run --all-files
```

The native desktop test is ignored by default and was explicitly run on Windows.
It saves screenshots under `target/photo-ui-screenshots/`. The reference's initial
page was rendered from an isolated copy under `target/` for visual comparison;
the supplied reference folder was not edited. Photo/Protocol tests, native desktop
test and package Clippy passed. Full-hook results and outstanding target-machine
acceptance are recorded in the local learning notes; existing repository Web
formatting and build-script line-ending failures are outside this port.

Real camera transfer, Chinese IME, long names, mixed-DPI monitors, physical
unplug/replug and a clean Windows installation remain target-machine acceptance
checks. A rendered fixture is not evidence that those checks passed.

## Live Server acceptance

The live test is ignored by default. A test administrator must create an isolated
Photo source and obtain a one-time pairing code through the existing API. It
uses `PHOTO_LIVE_URL`, `PHOTO_LIVE_CA`, `PHOTO_LIVE_SOURCE`,
`PHOTO_LIVE_WORKSPACE`, `PHOTO_LIVE_HOT_FOLDER`, `PHOTO_LIVE_ZIP` and, for a new
enrollment, `PHOTO_LIVE_CODE`. A paired, unfinished workspace can resume with
its original Windows-vault credentials and no new code. Do not put the code,
operator password, session cookie or signing seed into a fixture or report.

```powershell
cargo test -p natsume-photo-client --bin natsume-photo-client --locked live_server_ui_initialization_through_signed_zip -- --ignored --nocapture
```

This test invokes the same Slint callbacks used by the buttons. It does not
inject a renderer receipt, replace Windows credentials with a fake, or implement
an alternative capture/export path. Synthetic PNG/JPEG files simulate camera
arrivals; it is not a physical-camera or manual mouse/keyboard acceptance test.
After export it verifies the signature against the separately persisted local
authorization, ZIP image bytes and the reopened workspace. The test operator
must also compare the registered public key and initialization context with
Server records to establish the external trust binding.

On 2026-10-31 this passed against WSL Server with 418 teams, 237 school-logo
references and 232 unique PNG resources. A fresh initialization with the
optimized Server passed, as did recovery after a cold debug Server exceeded the
30-second request timeout. Server was then stopped: offline diagnosis and a
second export passed with byte-identical ZIP output. Evidence is retained in
the ignored `target/photo-ui-live-*` artifacts and local learning notes.
