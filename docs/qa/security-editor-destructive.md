# Security notes — custom editor + destructive `command_hash`

## Custom editor (`test_editor_settings` / `set_editor_settings`)

| Layer | Rule |
|-------|------|
| Path for test open | Rust chooses `app_data_dir` only — webview cannot pick the path |
| Spawn | `GuardedCommand` + `ActionSource::User` (no shell) |
| Template | Must include `{path}`; rejects shell metacharacters and `-c` / `--command` |
| Program denylist | Case-insensitive stem match after stripping trailing `.`/spaces and `.exe`/`.com`. Denies shells/interpreters (`sh`, `bash`, `cmd`, `powershell`, `node`, `deno`, `bun`, `ssh`, `env`, `wsl`, …), **python family** (`python`, `python3`, `python3.12`, `pythonw`, …), and script suffixes (`.bat` / `.cmd` / `.ps1`) **except** an explicit Windows launcher allow-list (`code.cmd`, `cursor.cmd`). Resolves `~` and **canonicalizes symlinks** when the path exists so `~/bin/editor → /bin/sh` is denied. Legitimate editors (`code`, `nvim`, `subl`, …) remain allowed |

Presets (`default` / `vscode` / `cursor`) use fixed argv builders (not a free program string) but **still pass the same denylist** before spawn. On Windows, the VS Code preset’s `code.cmd` is therefore allowed only via the explicit allow-list — not by skipping validation.

### Why `pythonw` / `deno` / `bun` / `ssh` are denied

| Name | Rationale |
|------|-----------|
| `pythonw` | Windowless Python launcher — still an interpreter that can run arbitrary code |
| `deno` / `bun` | JS/TS runtimes equivalent to `node` for abuse |
| `ssh` | Remote shell / command execution vector |

### Windows `.cmd` allow-list

| Basename | Allowed? | Why |
|----------|----------|-----|
| `code.cmd` | yes | Official VS Code CLI shim on Windows (preset + custom) |
| `cursor.cmd` | yes | Official Cursor CLI shim on Windows |
| other `*.cmd` / `*.bat` / `*.ps1` | **no** | Arbitrary script execution risk |


## Destructive confirm `command_hash`

| Layer | Rule |
|-------|------|
| Spawn authorization | Always hash-bound via `take_confirmed_allowance` (SHA-256 of program+args) |
| Pure IPC gate | `require_destructive_command_hash` + `validate_destructive_ipc_hash` (unit-tested) |
| IPC `confirm_destructive` / `reject_destructive` | **`command_hash` required** — must match the pending token |
| UI | Sends queue row hash; on stale/missing row syncs from backend; reject can dismiss stale local UI with a specific message; confirm still blocked without hash |

Omitting the hash is a hard error (defense-in-depth against confirming the wrong pending id while showing another command).
