# DS-03 Open in editor — manual per-OS checklist (PR-4)

Automated Playwright covers the UI button + `open_dead_symbol_in_editor` IPC
(path resolved from the SQLite index, not a raw webview path). Confirm live Tauri:

| OS | Default opener | Expected |
|----|----------------|----------|
| macOS | `open <file>` via guarded wrapper | File opens at/near line in default app |
| Windows | `explorer.exe <file>` (single argv; never `cmd /C start`) | File opens in default association |
| Linux | `xdg-open <file>` | File opens in default association |

Notes:
- Settings Editor preset (Default / VS Code / Cursor / custom) ships; default = OS opener above.
- Spoofed webview paths must be ignored — Rust resolves by project_id + name + kind.
