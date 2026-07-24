
# Dependency Policy

## Library first

Natsume implements single-contest seat/account/device semantics, explicit state/secret commands, command recovery, hardware-evidence policy, Gateway activation and Home transactions. It does not reimplement mature protocols, parsers, cryptography or OS interfaces.

Hardware collection order is `sysinfo` first, then `smbios-lib`, `raw-cpuid`, `procfs`, `udev` and `libsystemd` only where needed. The shared `machine-identity` crate remains pure; Linux I/O belongs to the privileged helper; startup comparison belongs to the daemon.

Other fixed boundaries include Tokio/Axum/Tower/SQLx, Quinn/rustls, Prost, zbus, Reqwest Unix socket, rustix, `csv`, HKDF + AEAD crypto crates, Caddy and nFPM. Session Agent GUI使用Slint；`.slint`在构建期编译，正式feature固定为winit backend + Skia renderer + std/compat/accessibility。产品代码不直接依赖winit、softbuffer、tiny-skia或cosmic-text。

## Error model

SNAFU is the only production Rust error framework. Modules define typed errors and private context selectors; stable HTTP/protocol/IPC error codes are explicit adapters. Binary entrypoints may use `snafu::Report`. `Whatever`, generic `Box<dyn Error>`, string-only protocol errors and transparent wrappers require a documented narrow boundary. `anyhow` and `thiserror` are rejected from the production graph.

## Import boundary

Imports accept one UTF-8 CSV (UTF-8 BOM allowed) per request, with exact header `seat,account,password`. The file is a complete authoritative snapshot. The first successful commit freezes the Seat universe; later files must contain exactly that set. Do not add `ImportSource`, multi-file joins, column mapping, XLSX/ODS readers or writers, delimiter guessing, or legacy-encoding detection.

## Secret boundary

- Persistent confidential payloads are AEAD ciphertext in Server/Client SQLite vault records.
- Random file root keys provide entropy; Client HKDF binds keys to `MachineHardwareId`.
- Root keys are not supplied through systemd credentials, environment variables, command-line arguments or package images.
- Password is absent from target state and may only cross the control channel in an explicit human-triggered `SYNC_SECRET` Command.

## TLS boundary

Enrollment uses a server-authenticated HTTPS configuration without a client certificate. Normal QUIC control uses a separate rustls configuration with mandatory Device client-certificate verification and 0-RTT disabled. Do not build an anonymous/optional-client-auth compatibility mode.

## Adapter rule

Create an adapter only at a privilege, platform, external-service or stable-contract boundary. Do not create wrappers that merely rename a third-party API.

## Locking

The implementation repository commits one `Cargo.lock`, one `pnpm-lock.yaml`, a pinned Rust toolchain, pinned pnpm/nFPM versions, a pinned Caddy version/digest and a release manifest.


## Session Agent runtime boundary

- System-wide XDG Autostart is the primary graphical-session entry; `graphical-session.target` is not the sole start condition.
- Bootstrap validates the current logind session and passes only an explicit environment allowlist through an owner-only atomic runtime descriptor.
- The desktop XDG Autostart entry directly owns the long-running Agent process. No Session Agent systemd user unit, bootstrap/run handoff, whole-environment import or display synthesis is allowed.
- LightDM/GDM are display managers, not GUI backends. Support is declared for complete desktop/session combinations.
- No GTK, Qt, WebKit, Electron, WebView, Node, Python, JVM, zenity, kdialog, yad or `xdg-open` runtime path.
- Wayland activation/focus is best-effort and reported as a presentation fact; it is never treated as a security lock.
- Desktop extensions are compile-time Rust adapters in the same package, not runtime plugins or downloaded scripts.

## Session Agent Slint feature policy

- Pin Slint and slint-build together; production disables default features.
- Allow only `std`, `compat-1-2`, `backend-winit`, `renderer-skia` and accessibility unless an ADR changes the support matrix.
- Reject Qt, interpreter, live-preview, system-tray, MCP and system-testing features in production dependency trees.
- Package tests inspect built ELF dependencies and clean-install closure; the source feature list does not replace target desktop tests.
- UI assets and `.slint` sources are package-owned build inputs; no runtime downloads or interpreted remote UI.
