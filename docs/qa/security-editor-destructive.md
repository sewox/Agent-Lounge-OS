# Security notes — custom editor + destructive `command_hash`

## Custom editor (`test_editor_settings` / `set_editor_settings`)

| Layer | Rule |
|-------|------|
| Path for test open | Rust chooses `app_data_dir` only — webview cannot pick the path |
| Spawn | `GuardedCommand` + `ActionSource::User` (no shell) |
| Template | Must include `{path}`; rejects shell metacharacters and `-c` / `--command` |
| Program denylist | Basename deny for shells/interpreters (`sh`, `python*`, `node`, `cmd.exe`, `powershell`, `curl`, …). Legitimate editors (`code`, `nvim`, `subl`, …) remain allowed |

Presets (`default` / `vscode` / `cursor`) use fixed argv builders and never accept a free program string.

## Destructive confirm `command_hash`

| Layer | Rule |
|-------|------|
| Spawn authorization | Always hash-bound via `take_confirmed_allowance` (SHA-256 of program+args) |
| IPC `confirm_destructive` / `reject_destructive` | **`command_hash` required** — must match the pending token |
| UI | Always sends the queue row’s `command_hash`; refuses invoke if missing |

Omitting the hash is a hard error (defense-in-depth against confirming the wrong pending id while showing another command).
