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
