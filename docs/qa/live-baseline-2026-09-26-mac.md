# Agent Lounge OS: LIVE (S2) QA baseline, real Tauri app on 3 displays

Run: 2026-09-26 18:28–18:32 (Europe/Istanbul). App: `tauri dev`, main 716f42c, kernel PID 60050, window id 19737. No restart, no git pull, no Chrome.
Navigation: sidebar AX links clicked via System Events (`AXPress`) at AX path window→group→group→scrollArea→AXWebArea→group[1]→group[3]→link[i]. No JS injection was used.
Captures: `screencapture -x -o -l 19737`, 1.8s after each navigation. Originals are on the Mac under `~/agent-tools/qa-live/<D>/*.png`. JPEGs (1600px) referenced as `qa-live/<D>/*.jpg` (transfer to `docs/qa/live-baseline-2026-09-26-mac/` when available).

| Display | Window frame actually used (System Events, top-left) | Capture px |
|---|---|---|
| D1 built-in 1512x982 @2x | (0,33) 1512x949 | 3024x1898 |
| D2 SAMSUNG 1920x1080 | (1512,30) 1920x1050 (macOS clamped y to 30: each display has its own menu bar) | 1920x1050 |
| D3 F27G3xTF portrait 1080x1920 | (3432,30) 1080x1890 (same clamp) | 1080x1890 |

Original window: (1512,30) 1920x1050, i.e. it was on **D2**, not D1. As instructed, it was restored to D1 at (0,33) 1512x949 (the original size doesn't fit on D1), on /dashboard.

Legend: % = rough estimate from the screenshot. L1 = panel span / content width (sidebar excluded). L3 = largest blank region / content area. L6 = panel width on portrait. "sidebar" = the global L5 defect below (G1).

## D1: built-in 14" (1512x949)
| Route | L1 | L2 | L3 | L4 | L5 | L6 | Other issues | Sev | Image |
|---|---|---|---|---|---|---|---|---|---|
| /dashboard | ✅ 100% | ✅ scrolls | ✅ <10% | ✅ | ❌ sidebar; Vault tree has its own h-scrollbar | n/a | "Decision: —" chip on every heartbeat row; "25/sayfa" TR; dotted İ ("TİME", "SEMANTİC") | P2 | qa-live/D1/dashboard.jpg |
| /stream | ✅ 100% | ✅ internal scroll | ✅ | ✅ | ❌ sidebar | n/a | same chip noise; 25 rows of pure heartbeats | P3 | qa-live/D1/stream.jpg |
| /vault | ✅ 100% | ✅ | ⚠️ ~15% (tree ends at ~50% of panel height) | ✅ | ❌ tree h-scrollbar (rows wider than column), sidebar | n/a | Experience log shows raw prompt dumps ("## Cross-Project Memory ### Tecrübeler …"); "405 düğüm / ref" TR | P2 | qa-live/D1/vault.jpg |
| /vault (project selected) | ✅ | ✅ | ⚠️ ~15% | ✅ | ❌ | n/a | Dead-symbol paths truncated with an ellipsis; "668 UYARI", "seçili:" TR | P2 | qa-live/D1/vault-project.jpg |
| /health | ❌ ~52% | ⚠️ frame to bottom, rows end at ~30% | ❌ ~48% right half blank + ~65% of panel interior empty | ❌ single 52% box | ❌ "100% indexed" chip collides with "sync: live" on long repo name | n/a | 668 dead symbols (see D-1); empty space unexplained | **P1** | qa-live/D1/health.jpg |
| /fleet | ❌ ~39% | ⚠️ frame to bottom, rows end at ~25% | ❌ ~61% right blank | ❌ single 39% box | ❌ sidebar | n/a | 5 rows, large empty panel | **P1** | qa-live/D1/fleet.jpg |
| /telemetry | ❌ ~62% | ✅ | ❌ ~38% right blank | ❌ 62% column | ❌ sidebar | n/a | Heavy TR/EN mix ("TOPLAM FAİLURE", "Ajan başına hata / failure", "MARKDOWN İNDİR") | **P1** | qa-live/D1/telemetry.jpg |
| /quotas | ⚠️ ~84% | ✅ | ⚠️ ~16% right strip | ✅ | ❌ "ok LOCAL" state badge wraps to 2 lines | n/a | "kullanınca", "abonelik", "host üzerinden" TR; Claude Desktop = Claude CLI identical numbers | P2 | qa-live/D1/quotas.jpg |
| /settings | ❌ ~70% | ✅ ~97% | ❌ ~30% right blank | ⚠️ 70% | ❌ "Bağlı Araçlar" description runs into the "Yeniden tara" button | n/a | "UI ÖLÇEĞİ / UI SCALE", "ROUTİNG POLİCY", "ENABLED" mix | P2 | qa-live/D1/settings.jpg |
| ⌘K palette | ✅ modal | – | – | ✅ intended centred modal | ✅ | n/a | "ACTİONS", "SWİTCH PROJECT", "ACTİVE" dotted İ; "↑↓ seç · ↵ çalıştır" TR | P3 | qa-live/D1/palette.jpg |

## D2: 32" SAMSUNG (1920x1050)
| Route | L1 | L2 | L3 | L4 | L5 | L6 | Other issues | Sev | Image |
|---|---|---|---|---|---|---|---|---|---|
| /dashboard | ✅ 100% (2 cols) | ❌ left column (NATS) ends at ~73% while right column scrolls | ❌ ~17% blank block under NATS panel | ✅ | ❌ Semantic Map squeezed into right column: "d…", "Users-macbookp…", "FleetPa…", "Onboard…" truncated; sidebar | n/a | Unbalanced 2-column grid | **P1** | qa-live/D2/dashboard.jpg |
| /stream | ✅ 100% | ✅ | ✅ | ✅ | ❌ sidebar | n/a | chip noise | P3 | qa-live/D2/stream.jpg |
| /vault | ✅ 100% | ✅ | ❌ ~20–25% (tree ends at ~55%, log at ~70% of panel height) | ✅ | ❌ tree h-scrollbar | n/a | raw prompt dumps in log | P2 | qa-live/D2/vault.jpg |
| /vault (project) | ✅ | ✅ | ⚠️ ~18% | ✅ | ❌ | n/a | dead-symbol list is a tiny 2-row scroller at top-right | P2 | qa-live/D2/vault-project.jpg |
| /health | ❌ ~40% | ⚠️ rows end at ~30% | ❌ ~60% | ❌ 40% box | ❌ chip collision | n/a | | **P1** | qa-live/D2/health.jpg |
| /fleet | ❌ ~30% | ⚠️ rows end at ~25% | ❌ ~70% | ❌ 30% box | ❌ sidebar | n/a | worst width ratio of all | **P1** | qa-live/D2/fleet.jpg |
| /telemetry | ❌ ~46% | ✅ | ❌ ~54% | ❌ 46% | ❌ sidebar | n/a | TR/EN mix | **P1** | qa-live/D2/telemetry.jpg |
| /quotas | ❌ ~63% | ✅ | ❌ ~37% | ❌ 63% | ❌ "ok LOCAL" wraps | n/a | | **P1** | qa-live/D2/quotas.jpg |
| /settings | ❌ ~53% | ✅ ~87% | ❌ ~47% | ❌ 53% | ❌ sidebar | n/a | | **P1** | qa-live/D2/settings.jpg |

## D3: 27" portrait (1080x1890)
The sidebar stays expanded (~20% of width). Header search is hidden and the model name is truncated to "gemma-2-2b-it-G".
| Route | L1 | L2 | L3 | L4 | L5 | L6 | Other issues | Sev | Image |
|---|---|---|---|---|---|---|---|---|---|
| /dashboard | ✅ | ❌ page ends at ~79%, blank below, no scroll | ❌ ~20% blank bottom | ✅ | ❌ KPI cards wrap badly ("amber alert" badge on 2 lines, Indexed Files subtitle on 4 lines); Semantic Map squashed to ~3 tree rows while space is unused below | ✅ ~100% | | **P1** | qa-live/D3/dashboard.jpg |
| /stream | ✅ | ✅ | ❌ ~30% blank inside panel under 25 rows | ✅ | ❌ sidebar | ✅ 100% | | P2 | qa-live/D3/stream.jpg |
| /vault | ✅ | ⚠️ frame to bottom, content ends at ~35–45% | ❌ ~55–60% of panel empty | ✅ | ❌ names truncated ("docker-compo…", "Users-macbookpro-Developer…") | ❌ inner tree/log columns stay side-by-side (~50% each) instead of stacking | This is Sercan's "Semantic Map fills only about half" issue at its worst | **P1** | qa-live/D3/vault.jpg |
| /vault (project) | ✅ | ⚠️ | ❌ ~55% | ✅ | ❌ "EXPERIENCE LOG" header wraps and collides with "FİLTER · USERS-MACBOOKPRO-…"; pager "1/34 · 405" wraps | ❌ | | **P1** | qa-live/D3/vault-project.jpg |
| /health | ❌ ~77% | ⚠️ rows end at ~20% | ❌ ~80% of panel empty | ❌ | ❌ chip collision | ❌ ~77% | | **P1** | qa-live/D3/health.jpg |
| /fleet | ❌ ~58% | ⚠️ rows end at ~15% | ❌ ~85% | ❌ 58% box | ❌ sidebar | ❌ ~58% | | **P1** | qa-live/D3/fleet.jpg |
| /telemetry | ⚠️ ~91% | ✅ | ❌ ~55% of report panel empty | ✅ | ❌ sidebar | ❌ ~91% | | P2 | qa-live/D3/telemetry.jpg |
| /quotas | ✅ | ✅ | ✅ | ✅ | ❌❌ **horizontal overflow**: STATE column cut off, RESET clipped at right edge, h-scrollbar; cells wrap to 2–3 lines | ✅ 100% (but overflowing) | | **P1** | qa-live/D3/quotas.jpg |
| /settings | ✅ | ❌ ends at ~50%, no scroll | ❌ ~50% blank bottom | ✅ | ❌ "Yeniden tara" button wraps to 2 lines | ✅ ~98% | | P2 | qa-live/D3/settings.jpg |

## Failure matrix (❌ or ⚠️ on L1–L4/L6; L5 fails everywhere because of G1)
- /health: D1 D2 D3 · /fleet: D1 D2 D3 · /telemetry: D1 D2 D3 · /settings: D1 D2 D3 · /quotas: D1 (borderline) D2, D3 (overflow)
- /vault: D1 (borderline) D2 D3 · /dashboard: D2 D3 · /stream: D3 only (L3)

## Global defects (all displays)
- **G1 (L5, P2):** the sidebar header "+ New Node" button overlaps the "AL-OS CORE" title (clipping it to "AL-OS CORI"), and "v0.1.0 · ready" wraps to 2 lines.
- **G2 (P2):** `<html lang="tr">` (src/app/layout.tsx:44) + CSS `uppercase` turns English labels into dotted-İ forms: "TİME", "KİND", "LİMİT", "SEMANTİC MAP + EXPERİENCES", "ACTİVE DAEMONS", "CRİTİCAL QUOTAS", "ROUTİNG POLİCY", "LAYA DECİSİON", "FİLTER". The AX tree exposes the same strings, so screen readers read them too.
- **G3 (P2):** Turkish and English are mixed on every page (panel titles EN, helper text TR, "25/sayfa", "seçili:", "668 UYARI", "405 düğüm", "Kaydet", "Yeniden tara", "kullanınca").
- **G4 (P1):** fixed-width, left-aligned single panels (Health, Fleet, Telemetry, Quotas, Settings) don't use the width on D2, and don't reach 95% on D3.
- **G5 (P3):** the Next.js dev indicator ("N" bubble) sits on the sidebar footer over "Docs" (dev build only).

## Issues visible only in the live app (not in a browser mock), prioritised
1. **P1, data inconsistency / likely truncation:** Vault reports "66 files · 800 edges · 400 nodes" (+"405 düğüm"), while the experience texts from the same memory bridge say `nodes=2286 edges=7958 dead=0`. The round 400/800 values suggest a query LIMIT cap. Dead Symbols shows **668** (Dashboard, Health, Telemetry "kalan 668") while the index says `dead=0`. Either the counter or the cap is wrong. No mock values (127, 12/41/74) were seen. MSG/MIN 12 matches 2 heartbeats per 10s.
2. **P1:** D3 /quotas horizontal overflow and D3 /vault non-stacking columns only show up at the real portrait width with real row content (long plugin names, "kullanınca · 5g 6s").
3. **P1:** real long identifiers (`Users-macbookpro-Developer-Agent-Lounge-OS`) break layouts: truncation in the D2/D3 dashboard Semantic Map, the Health chip collision, the D3 vault header collision, and wrapping in the KPI card subtitle.
4. **P2:** the Experience Log renders raw prompt/markdown dumps (`## Cross-Project Memory ### Tecrübeler - [e2e-verify|…] (score=0.33) …`) and internal Turkish errors (`memory_bridge hata: repo_path çözümlenemedi: agent-lounge-os`) as user-facing text.
5. **P2:** the Event Stream is 100% heartbeats (`lounge.bus.heartbeat` / `lounge.workers.heartbeat`), each with an empty "Decision: —" chip. Real traffic is buried; heartbeats should be filtered or collapsed by default. Latency "—"/idle and Laya Decision "—" are shown without explanation on the Dashboard and Telemetry.
6. **P3:** Claude Desktop and Claude CLI show identical quota numbers (0% 5s · 33% 7g, same reset). Possibly one account shown twice.
7. **P3:** backend-legacy / frontend-new are indexed as "1 files · 1 AST nodes · 0 edges" (grandtest fixtures?). They look like placeholder projects in the live tree and in the palette.
8. **P3:** window-manager effects: each display has its own menu bar, so usable height is 1050/1890, not 1080/1920 (NSScreen visibleFrame misreports this). The native title bar costs another ~28pt.

## Automation notes
- The screen was unlocked at the start and stayed unlocked. `caffeinate -d -i -u -t 1800` (PID 79466) was started with nohup+disown, but it had already exited by the end (it was likely killed when the Shell session closed), so it probably wasn't holding the display awake. No lock happened.
- A recursive AX walk over the whole webview was too slow (>70s, killed). Direct index paths answer in ~0.2s. Helpers are kept on the Mac: `~/agent-tools/qa-live/{nav.js,move.js,vaultsel.js,run.sh,axlvl.js}`. Usage: `./run.sh D2 1512 0 1920 1080`.
- The app has a second, hidden window, "Codebase Memory · 3D Graph" (id 19910). It was not touched.
- Selecting the project in the Vault tree is app UI state (it sets the log filter "FİLTER · USERS-…" / "seçili:"). It was left selected; it's a view filter only.

## Harness follow-ups (PR-0)
Automated L1–L6 hardened to catch G1/G4 and sparse panel interiors; dedicated cases: EX-14 (LIMIT-as-total), EX-15 (raw log dumps), SR-02 (heartbeat filter), X-02 (dotted İ). See `docs/qa/baseline-2026-09-26.md` § Live vs automated.
