# Feature gallery captures

The root [feature gallery](../../README.md#feature-gallery) shows seven current Operator panel views and two native Session Agent previews. These are documentation illustrations with synthetic data, not live-contest screenshots or evidence of backend integration, device convergence, GNOME Kiosk behavior, first-frame readiness, or deployment and scale acceptance.

## Capture contract

- Keep every PNG at **2880 × 2160**: a logical **1440 × 1080** interface rendered at **2×** scale.
- Capture the actual current UI. Do not redraw it, override its layout, resize an old screenshot, crop it, or stretch it to fit the gallery.
- Keep the root gallery as one equal-column table. Update its captions and image links when pages or responsibilities change; Bindings is now part of Devices, not a separate gallery page.
- Use synthetic accounts, teams, schools, devices, invitations, and network addresses. Never capture real contest data, passwords, invitation/reset bearer links, or an existing personal desktop.

## Web panel

Use the repository's Node.js 24.1.0 and pnpm 11.x, installed workspace dependencies, and the Chromium version matching the locked Playwright dependency. From the repository root:

```bash
pnpm --dir web build
node web/scripts/capture-gallery.mjs
```

The opt-in [capture helper](../../web/scripts/capture-gallery.mjs) serves the production build on its own ephemeral loopback port, opens a fresh Chromium context, and intercepts API calls with schema-typed fixtures. It does not start or contact Server. It uses a fixed clock, light theme, English locale, UTC timezone, and the capture geometry above; unknown API calls, writes, external requests, page errors, loading states, and horizontal overflow fail the capture. Browser and preview resources are closed afterward. It adds no CI job or test gate.

The helper refreshes `preparation.png`, `enrollment.png`, `accounts.png`, `seats.png`, `devices.png`, `targets.png`, and `users.png`. The fixtures intentionally include bound/unbound seats, active/offline/disabled devices, a closed enrollment window with pending reviews, and administrator/viewer accounts without exposing issued links. Device bindings, Unbind actions, lifecycle/connection/binding facets, and the compact Emergency tool are shown on Devices. System fonts and browser versions may affect pixels on another machine.

## Native Session Agent previews

Build the existing, development-only [UI probe](../../crates/natsume-workstation-agent/examples/ui_probe.rs) from current source:

```bash
cargo build -p natsume-workstation-agent --example ui_probe --locked
```

Run the following modes separately in a dedicated virtual display, never on an existing personal desktop. The captures here use Xvfb with a fullscreen-capable Openbox window manager, a 2880 × 2160 output, and `WINIT_X11_SCALE_FACTOR=2` with the Skia software renderer. This is a preview setup, not the deployed Wayland session; no Agent runtime or D-Bus behavior is validated by these images. Point `DISPLAY` at that isolated display before running:

```bash
SLINT_BACKEND=winit-skia-software WINIT_X11_SCALE_FACTOR=2 cargo run -p natsume-workstation-agent --example ui_probe --locked -- binding_prompt
SLINT_BACKEND=winit-skia-software WINIT_X11_SCALE_FACTOR=2 cargo run -p natsume-workstation-agent --example ui_probe --locked -- waiting_offline
```

After each preview is fully rendered, capture the isolated display without postprocessing as `seat-setup.png` or `waiting.png`, respectively, then stop the probe with Ctrl-C. For example, ImageMagick's `import -display "$DISPLAY" -window root docs/screenshots/seat-setup.png` captures only the display you explicitly selected. Stop the owned virtual display and window manager when finished. The Waiting preview uses the probe's sample team and fallback school icon; a development preview may lack the package-provided author avatar.

Visually check all nine images at full resolution before replacing the gallery: current navigation and account menu, readable tables and controls, correct active page, no loading/error overlays or clipped content, and the expected native team/seat/offline presentation. Native preview captures do not substitute for target-image acceptance.
