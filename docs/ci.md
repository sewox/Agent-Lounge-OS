# CI status checks

Branch protection (owner-managed) can require these **16** check names. They must report on every PR — no `paths` / `paths-ignore` filters that would skip the job on unrelated changes.

| # | Check name | Workflow |
|---|------------|----------|
| 1 | CI | `ci.yml` (aggregate gate) |
| 2 | Frontend | `ci.yml` |
| 3 | Frontend (macos-latest) | `qa-cross-platform.yml` |
| 4 | Frontend (windows-latest) | `qa-cross-platform.yml` |
| 5 | Rust | `ci.yml` |
| 6 | Rust (macos-latest) | `qa-cross-platform.yml` |
| 7 | Rust (windows-latest) | `qa-cross-platform.yml` |
| 8 | Workers | `ci.yml` |
| 9 | QA static gates | `ci.yml` |
| 10 | MCP probe (ubuntu-latest) | `ci.yml` |
| 11 | MCP probe (macos-latest) | `ci.yml` |
| 12 | MCP probe (windows-latest) | `ci.yml` |
| 13 | Playwright e2e | `qa-e2e.yml` |
| 14 | Playwright smoke (macos-latest) | `qa-e2e-smoke.yml` |
| 15 | Playwright smoke (windows-latest) | `qa-e2e-smoke.yml` |
| 16 | Build AppImage + deb | `linux-bundle.yml` |

## Notes

- **Zero-failure policy:** no `continue-on-error`, no `|| true`, no `#[ignore]`, no expected-fail in PR CI.
- **Concurrency:** PR workflows use `cancel-in-progress` on the same ref so superseded pushes free runners.
- **Not required:** `nightly-laya.yml` (`Prove live_laya_infer filter` / `Laya live infer`) is schedule / `workflow_dispatch` (and path-filtered on PRs that touch its wiring only). Do not add those names to branch protection.

## apt mirror stalls

**Build AppImage + deb** (`linux-bundle.yml` → job `linux-bundle`) installs GTK/WebKit build deps via apt. Ubuntu mirrors on GitHub-hosted runners can stall mid-`apt-get update` / `install` (seen 2026-10-07: `azure.archive.ubuntu.com` timed out / was ignored, then the run hung ~40 minutes on `https://archive.ubuntu.com` until cancelled).

`scripts/ci/install-linux-build-deps.sh` is **Ubuntu/Debian apt only** (the ubuntu-22.04 bundle job). It reads `/etc/os-release` (`ID` / `ID_LIKE`, CRLF-tolerant) and treats a host as apt-based only when those fields contain `debian` or `ubuntu`. On any other distro it refuses immediately with `::error::`, naming the detected ID and package manager (`dnf` / `yum` / `zypper` / `pacman` / `apk` / `unknown`). Self-tests: non-apt hosts (and macOS/Windows) print `skip: apt gate not applicable on <ID> (package manager: …)`; a local Debian/Ubuntu machine missing `apt-config` prints `skip: apt-config missing on <ID> (non-CI); -o opts gate not run …` — never a bare `ok`. On Debian/Ubuntu GitHub Actions, a missing `apt-config` is a broken runner and fails with `::error::`.

Mitigations (step name unchanged):

1. **apt options** via `/etc/apt/apt.conf.d/zzzz-agent-lounge-ci-retries` (must be lexically last; asserted at runtime) plus matching `apt-get -o` flags: `Acquire::Retries "5"`, HTTP/HTTPS/FTP timeouts `30`s, `DPkg::Lock::Timeout "120"`. Effective config is checked with `apt-config -o … dump` (same `-o` set as `apt-get`).
2. **Step `timeout-minutes: 26`** so a stall fails fast instead of consuming the job’s 90-minute budget, while still allowing 3 full attempts including `timeout -k 15` kill wait (update 180s+15 + install 290s+15 + backoff 10/30 → worst case 1540s < 1560s).
3. **Bounded retry** (max 3 attempts, backoff 10s then 30s): `timeout -k 15` around update (180s) and install (290s). On retry: `apt-get clean`, clear partial lists, `dpkg --configure -a` (cleanup failures emit `::error::`), mirror flip from a pristine sources backup (`cp`/`sed` failures also emit `::error::`). Final failure emits `::error::` and exits non-zero — no `|| true`, no `continue-on-error`.
4. **Mirror fallback** on retry: back up apt sources once; attempt 2 switches `azure.archive.ubuntu.com` → `archive.ubuntu.com`; attempt 3 restores the backup (then switches archive → azure only if the backup had no azure). Sed uses escaped dots and a `//` anchor.

Prove the drop-in is active in CI logs: drop-in contents + `apt-config` with `-o` opts showing `Acquire::Retries "5"` and Timeout `"30"`, and the drop-in listed as lexically last under `/etc/apt/apt.conf.d`.

**Follow-up (out of scope here):** the same raw `apt-get` pattern still exists in `ci.yml` (Rust job) and `nightly-laya.yml` (two jobs).
