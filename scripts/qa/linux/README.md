# S2-Linux — live QA on Ubuntu-like desktop

Manual live pass for Agent Lounge OS on **Linux (X11)**. Complements Mac S2 (`scripts/qa/mac/`) and Windows (`scripts/qa/windows/`).

## Artifacts

| Source | What |
|--------|------|
| GitHub Actions → **Linux bundle** | Artifact **`agent-lounge-linux`** (`.deb` + `.AppImage` + this docs) |
| Workflow | [`.github/workflows/linux-bundle.yml`](../../../.github/workflows/linux-bundle.yml) |
| Triggers | `workflow_dispatch`, push to `main` (not on PRs; non-blocking) |

Runtime packages and NATS/LMR/Ollama expectations: **[RUNTIME_DEPS.md](./RUNTIME_DEPS.md)**.

## Target environment

- Distro: Ubuntu 22.04 / 24.04-like (or Debian with webkit2gtk 4.1)
- Display: **X11**
- Primary window: **1280×800** (app default in `tauri.conf.json`)
- Portrait simulation: resize the same window to **≈1080×1920** or **800×1280** (no second monitor required)

## Quick start

1. Install runtime deps from `RUNTIME_DEPS.md`.
2. Download `agent-lounge-linux` from Actions (or build locally: `npm ci && bash src-tauri/scripts/prepare-sidecar.sh --stub && npm run tauri -- build -- --bundles deb appimage`).
3. Install `.deb` or run AppImage.
4. Follow **[CHECKLIST.md](./CHECKLIST.md)**; fill **[QA_Report.md](./QA_Report.md)**.
5. Attach screenshots under a dated folder if filing a UI PR.

## Related plan

- QA plan §10.2 Cross-platform · S2 live templates
- Automated layout (Playwright D0 ≈1280×800, D3 portrait) in `e2e/` — live pass catches Tauri/OS-only issues
