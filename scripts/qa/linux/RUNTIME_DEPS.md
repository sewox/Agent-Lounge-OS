# Agent Lounge OS — Linux runtime dependencies (S2-Linux)

Target: Ubuntu-like desktop, **X11**, window **1280×800** (plus portrait resize).

## Required to launch the UI (Tauri / WebKit)

Install on the test machine (Debian/Ubuntu package names):

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-0 \
  libgtk-3-0 \
  libayatana-appindicator3-1 \
  librsvg2-2 \
  libssl3 \
  ca-certificates \
  xdg-utils \
  libfuse2
```

| Library | Why |
|---------|-----|
| `libwebkit2gtk-4.1-0` | Tauri 2 webview |
| `libgtk-3-0` | Windowing / menus |
| `libayatana-appindicator3-1` | Tray / indicator (if used) |
| `libfuse2` | Running **AppImage** (`.deb` does not need FUSE) |
| `xdg-utils` | `xdg-open` for “Open in editor” (§10.2 DS-03) |

Wayland-only sessions: prefer an X11 session (or XWayland) for this S2 pass — checklist assumes X11.

## Kernel / services — what starts without extras

The shell **starts without** host Ollama and without a pre-installed NATS binary. Missing services show as degraded in Health / banners; the window should still open.

| Component | Required to open app? | Notes |
|-----------|----------------------|--------|
| **NATS** (`nats-server` on PATH, `127.0.0.1:4222`) | No | Kernel supervisor tries to spawn/recover NATS. Without it: Event Stream / Fleet / task bus stay empty or degraded. Install from [nats.io](https://nats.io) or distro package if you need bus features. |
| **Host Ollama** (`:11434`) | No | Intentionally unused. Do **not** point Lounge at host Ollama. |
| **Lounge LMR** (`127.0.0.1:18790`, `data/lmr`) | No | Local model runner for inference/quota paths. UI works without it; LMR-backed actions fail soft. |
| **Laya weights** | No | Optional DecisionGate model under `~/.local/share/AgentLounge/models/laya`. Falls back when missing. |
| **codebase-memory-mcp** sidecar | Yes (full S2) | `linux-bundle.yml` fetches DeusData `v0.11.0` linux-amd64 into the package. Artifact `agent-lounge-linux` includes a real sidecar (`BUILD_INFO.txt` has size/version). Stub fallback publishes `agent-lounge-linux-ui-only` only. |

## Recommended for a fuller S2-Linux pass

```bash
# Optional bus
# install nats-server → ensure `nats-server` on PATH

# Optional: real memory bridge binary before rebuild
# bash src-tauri/scripts/prepare-sidecar.sh
```

## Installers from CI

Download the GitHub Actions artifact **`agent-lounge-linux`** from workflow **Linux bundle** (`linux-bundle.yml`).

- **`.deb`**: `sudo apt install ./agent-lounge-os_*.deb` (or `dpkg -i` then fix deps)
- **`.AppImage`**: `chmod +x *.AppImage && ./Agent*.AppImage`

See [README.md](./README.md) for the live checklist.
