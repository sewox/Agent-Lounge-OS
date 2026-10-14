# Agent Lounge OS — Design System (PR-DS-0)

Foundation every later UI PR uses: principles, primitives, states, viewport rules, and shared layout gates.

## Principles

1. **Full-bleed content.** App pages fill the content column (sidebar excluded). No `max-width` centering containers on app surfaces. Modals/palettes may center.
2. **Container-driven layout.** Panel count follows **container width**, not device labels. Portrait is an aspect-ratio mode, not a separate product.
3. **Shell chrome is fixed.** Top status bar + left rail + content. Content owns scroll; chrome does not introduce horizontal overflow (L5).
4. **One composition per surface.** Prefer list | detail | inspector over stacked card dashboards.
5. **Honest state language.** Halted ≠ error. Empty ≠ loading. Prefer explicit StatusPill / banner affordances.
6. **Keyboard & a11y first.** Stats, filters, and drawers are operable with keyboard; interactive numbers have visible active state and aria labels.
7. **Motion respects preference.** Decorative motion (hatch shimmer, pulses) yields to `prefers-reduced-motion: reduce`.

## Shell

| Region | Role |
|--------|------|
| Top status bar | Brand, search/palette, quotas, approvals, locale |
| Left rail | Primary nav + daemon health |
| Content (`main`) | Route body — must fill remaining width/height |

## 1 / 2 / 3-panel grid

Component: `PanelGrid` (`src/components/design-system/panel-grid.tsx`).

Slots: **list** | **detail** | **inspector**.

| Container | Mode | Behavior |
|-----------|------|----------|
| width &lt; 1200px | `one` | List fills content; detail/inspector open as **drawers** |
| 1200–1699px | `two` | List + detail side-by-side; inspector as drawer |
| ≥ 1700px | `three` | List + detail + inspector |
| aspect &lt; 1 (portrait) | `portrait-stack` | List on top, detail below, **draggable splitter**; inspector as drawer |

Breakpoints are measured on the grid container via `ResizeObserver`, not on `window` alone.

## StatsStrip

Component: `StatsStrip`. Every number is a **button** that sets the active list/FilterBar category (`onFilterChange`). Requirements:

- Keyboard focusable; Enter/Space activate
- `aria-pressed` / `aria-label` reflecting category + count
- Visible active state (border/background), not color alone

Wire to a FilterBar (documented below) or local list filter until FilterBar ships.

## Halted visual language

Components: `StatusPill` (`tone="halted"`) and `HaltedBanner`.

- **Meaning:** deliberately stopped (operator Halt), not a crash/error
- **Look:** diagonal striped/hatched field + red banner strip
- **Contrast:** error uses solid error-container chrome without hatch
- **Motion:** hatch may animate slowly; disabled under `prefers-reduced-motion`

Halt **logic** is PR-Control-1. This package ships visual state only.

## Status tones

| Tone | Use |
|------|-----|
| `ok` | Healthy / running |
| `warn` | Degraded / limited |
| `error` | Failure / crash |
| `halted` | Deliberately stopped |
| `neutral` | Idle / unknown |

## Documented primitives (not built in PR-DS-0)

| Primitive | Intent | Follow-up |
|-----------|--------|-----------|
| **PageHeader** | Title + actions row for a route | Later UI PRs |
| **FilterBar** | Category/text filters; StatsStrip writes into it | Later UI PRs |
| **MasterDetail** | Opinionated list↔detail binding on top of PanelGrid | Later UI PRs |
| **StickyActionBar** | Bottom actions that stay visible while content scrolls | Later UI PRs |
| **EmptyState** | Generic empty (IndexEmptyState already covers index/map) | Later UI PRs |
| **Timeline** | Chronological event spine | Out of scope here |

## Viewport matrix (D0–D3)

| ID | Size (CSS px) | Notes |
|----|---------------|-------|
| D0 | 1280×800 | CI baseline |
| D1 | 1512×982 @2x | Built-in laptop |
| D2 | 1920×1080 | Wide desktop |
| D3 | 1080×1920 | Portrait — panels stack (L6) |

PR-DS-0 layout helpers must be exercised at **D1, D2, and D3** (and remain green on D0 via the existing suite).

## Layout gates L1–L6

Canonical implementation: `e2e/helpers/layout.ts` (`measureLayout`, `assertLayoutL1toL6`). **Do not redefine thresholds in product code.**

| Gate | Pass condition (extracted from helper) |
|------|----------------------------------------|
| **L1** | Union of `main [data-qa=panel]` widths ≥ **85%** of content width (viewport − sidebar) |
| **L2** | Panel union bottom ≥ **85%** of viewport height **or** page/main scrolls; tall panels (≥50% vh) must not be sparse (interior fill ≥ **45%**) |
| **L3** | Largest empty 8×8 cell block over content ≤ **15%** of content area |
| **L4** | No single panel narrower than **70%** of content (centered or left-aligned unused width — live G4) |
| **L5** | No horizontal page overflow; sidebar children stay inside rail; `+ New Node` must not overlap `AL-OS CORE` |
| **L6** | Portrait only: every panel width ≥ **95%** of content width (stack, don’t split side-by-side) |

Shared assert API: `assertLayoutL1toL6(page, route, expect)` — usable from any Playwright spec.

## Reference surface

`/ds-lab` — proof page for PanelGrid + StatsStrip + Halted language. Not in primary nav; opened by URL in e2e. **No broad page migration in this PR.**

## Testing

- Unit: grid breakpoint resolver + StatsStrip filter clicks (`src/components/design-system/*.test.ts`)
- E2E: `e2e/tests/design-system.spec.ts` uses L1–L6 helpers at D1/D2/D3 and checks StatsStrip / Halted affordances

## Out of scope (this PR)

Halt control logic, review queue, Vault card, dead-code screen, timeline UI, i18n/left-menu responsiveness (PR-i18n-Platform-1).
