# Commit History

## feat: add GUI bind mode with zenity prompt via --prompt flag

- Context-Id: ``
- Branch: ``
- Timestamp: ``
- Files:
  - `src/main.rs`
  - `src/client.rs`
  - `src/client/bind.rs`
  - `src/client/desktop.rs`
- Decisions:
  - {'title': 'Re-exec detach instead of libc::fork', 'rationale': 'Zero unsafe code, zero new dependencies. Child is a fresh exec so no post-fork Rust state hazards. Uses nightly process_setsid feature.', 'tradeoffs': ['Config re-parsed in child (negligible cost)', 'Bind results not visible in pssh output, only in logs + zenity feedback'], 'assumptions': ['Nightly toolchain is acceptable', 'process_setsid feature is stable-track'], 'rejected_alternatives': [{'option': 'libc::fork() + setsid()', 'reason': 'Requires unsafe blocks, post-fork Rust state issues'}, {'option': 'nix crate', 'reason': 'Still unsafe under the hood, adds dependency'}, {'option': 'daemonize crate', 'reason': 'Overkill, hides unsafe, adds dependency'}], 'side_effects': ['New hidden --_bg CLI flag for internal use']}
  - {'title': 'Cross-user GUI via runuser + loginctl session detection', 'rationale': "setuid root binary must not directly connect to stu's X11/Wayland. runuser -u stu ensures zenity runs with correct user credentials and session access.", 'tradeoffs': ['Depends on loginctl, runuser, zenity being available on target'], 'assumptions': ['Target systems use systemd/logind', 'player_user has active local graphical session'], 'rejected_alternatives': [{'option': 'xhost + root direct display access', 'reason': 'Security risk: exposes X11 to all local users'}, {'option': 'xauth cookie copy to root', 'reason': 'Cookie leakage grants full display access'}], 'side_effects': ['SAFE_PATH includes sbin for runuser resolution']}

## chore(assets): tighten firefox contest browser policy

- Context-Id: ``
- Branch: ``
- Timestamp: ``
- Files:
  - `assets/configure_client.sh`
- Decisions:
  - {'title': 'Apply stricter Firefox enterprise defaults', 'rationale': 'Keep contest browsers focused on the contest site and suppress first-run and account-related flows.', 'tradeoffs': ['Reduces browser flexibility for operators during setup'], 'assumptions': ['Client machines honor Firefox enterprise policies'], 'rejected_alternatives': [], 'side_effects': ['Disables telemetry, studies, and default-browser prompts']}

## chore(assets): tighten firefox contest browser policy

- Context-Id: ``
- Branch: ``
- Timestamp: ``
- Files:
  - `assets/configure_client.sh`
- Decisions:
  - {'title': 'Apply stricter Firefox enterprise defaults', 'rationale': 'Keep contest browsers focused on the contest site and suppress first-run and account-related flows.', 'tradeoffs': ['Reduces browser flexibility for operators during setup'], 'assumptions': ['Client machines honor Firefox enterprise policies'], 'rejected_alternatives': [], 'side_effects': ['Disables telemetry, studies, and default-browser prompts']}

## fix(client): remove lightdm seat header on cleanup

- Context-Id: ``
- Branch: ``
- Timestamp: ``
- Files:
  - `src/client/session.rs`
- Decisions:
  - {'title': 'Strip LightDM seat section during cleanup', 'rationale': 'Avoid leaving duplicate [Seat:*] sections behind when autologin is toggled multiple times.', 'tradeoffs': ['Removes the generic seat header even if other options were manually placed under it'], 'assumptions': ['The managed client config owns this autologin section'], 'rejected_alternatives': [], 'side_effects': ['Cleanup now removes both the autologin keys and the seat header']}

## fix(assets): configure caddy admin before reload

- Context-Id: ``
- Branch: ``
- Timestamp: ``
- Files:
  - `assets/configure_client.sh`
- Decisions:
  - {'title': 'Prepend Caddy global options during client bootstrap', 'rationale': 'Set the admin endpoint to localhost:20190 and disable automatic HTTPS immediately after installing Caddy so later reload operations use the expected control socket and do not fail on HTTPS automation.', 'tradeoffs': ['Restarts Caddy during client bootstrap'], 'assumptions': ['The installed Caddy package creates /etc/caddy/Caddyfile before this step'], 'rejected_alternatives': [], 'side_effects': ['Caddy global options are inserted only if the admin line is absent']}

## feat: update contest import and print tooling

- Context-Id: `04f9d436-a06d-48f5-bca6-63e56bd67564`
- Branch: `main`
- Timestamp: `2026-05-03T01:32:08.084384+00:00`
- Files:
  - `README.md`
  - `assets/print_client.py`
  - `assets/print_typst.py`
  - `data_preprocess/README.md`
  - `data_preprocess/data_preprocess.py`
  - `data_preprocess/pyproject.toml`
  - `data_preprocess/upload_organization_logo.py`
  - `data_preprocess/uv.lock`
- Decisions:
  - Support contest XLSX imports with category-based team groups and combined Chinese/English team display names.
  - Preserve XLSX string values and common Excel error literals during preprocessing so import fields are not coerced away.
  - Normalize organization logos to 64x64 PNG before uploading to DOMjudge.
  - Document current client/server deployment, judgehost setup, data preprocessing, and print helper responsibilities.
  - Add DOMjudge-side Typst PDF generation and print-station PDF receiver scripts for contest printing.
