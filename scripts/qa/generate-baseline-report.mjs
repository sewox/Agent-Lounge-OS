#!/usr/bin/env node
/**
 * Build docs/qa/baseline-2026-09-26.md from Playwright JSON + layout-metrics.jsonl.
 */
import fs from "node:fs";
import path from "node:path";

/** Playwright JSON + layout metrics live under ignored test-results/ (CI artifact). */
const ARTIFACT_ROOT = path.resolve("test-results", "qa-baseline");
const REPORT_JSON = path.join(ARTIFACT_ROOT, "playwright-report.json");
const METRICS = path.join(ARTIFACT_ROOT, "layout-metrics.jsonl");
const OUT_MD = path.resolve("docs/qa/baseline-2026-09-26.md");

const PR_MAP = [
  { re: /^EX-0[2-7]|^EX-10|^DS-05|^DS-07|^DS-08/, pr: "PR-1 Backend-Core" },
  { re: /^SH-0[237]|^SH-08|^DB-0[124]|^HM-0[125]|^ST-01|^X-0[14]|LAYOUT|EX-LAYOUT/, pr: "PR-2 Cleanup-Shell" },
  { re: /^EX-0[1389]|^EX-04|^CP-02/, pr: "PR-3 Vault-Experience" },
  { re: /^DS-0[1-8]|^HM-03|^HM-04|^DS-LAYOUT/, pr: "PR-4 Health-Symbols" },
  { re: /^ST-0|^GR-|^OB-|^FL-/, pr: "PR-5 System-Settings" },
];

function mapPr(title) {
  for (const row of PR_MAP) {
    if (row.re.test(title)) return row.pr;
  }
  if (/LAYOUT/i.test(title)) return "PR-2 Cleanup-Shell";
  return "—";
}

function loadTests(suite, acc = []) {
  if (!suite) return acc;
  for (const spec of suite.specs || []) {
    for (const t of spec.tests || []) {
      const result = (t.results || [])[0] || {};
      acc.push({
        title: spec.title,
        project: t.projectName,
        expectedStatus: t.expectedStatus,
        status: t.status,
        error: (result.error?.message || "").split("\n")[0].replace(/\x1b\[[0-9;]*m/g, ""),
        annotations: (t.annotations || []).map((a) => `${a.type}:${a.description || ""}`),
      });
    }
  }
  for (const child of suite.suites || []) loadTests(child, acc);
  return acc;
}

const json = JSON.parse(fs.readFileSync(REPORT_JSON, "utf8"));
const rows = [];
for (const suite of json.suites || []) loadTests(suite, rows);

let pass = 0;
let expectedFail = 0;
let unexpectedFail = 0;
let skipped = 0;

for (const r of rows) {
  if (r.status === "skipped" || r.expectedStatus === "skipped") {
    skipped++;
    continue;
  }
  if (r.status === "expected" && r.expectedStatus === "passed") pass++;
  else if (r.status === "expected" && r.expectedStatus === "failed") expectedFail++;
  else if (r.status === "unexpected") unexpectedFail++;
  else if (r.status === "passed") pass++;
  else if (r.status === "failed" && r.expectedStatus === "failed") expectedFail++;
  else unexpectedFail++;
}

const metrics = fs.existsSync(METRICS)
  ? fs.readFileSync(METRICS, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l))
  : [];

const layoutViolations = metrics.filter(
  (m) => !m.l1_pass || !m.l2_pass || !m.l3_pass || !m.l4_pass || !m.l5_pass || m.l6_pass === false,
);

const blockerExpected = rows.filter((r) => {
  const isB =
    /\[B\]|^SH-0[128]|^DB-0[124]|^EX-|^DS-0[125]|^HM-0[1235]|^ST-01|^X-0[34]|LAYOUT/i.test(r.title);
  const failedAsExpected = r.expectedStatus === "failed" && r.status === "expected";
  return isB && failedAsExpected;
});

const lines = [];
lines.push("# QA Baseline — 2026-09-26 (PR-0)");
lines.push("");
lines.push("| Field | Value |");
lines.push("|-------|-------|");
lines.push("| Commit base | `15e19e9` (main) |");
lines.push(`| Generated | ${new Date().toISOString()} |`);
lines.push("| Suite | Playwright e2e (S1) + S4 gates |");
lines.push(
  `| Counts | **pass=${pass}** · **expected-fail=${expectedFail}** · **unexpected-fail=${unexpectedFail}** · skipped=${skipped} · total=${rows.length} |`,
);
lines.push(`| Layout metrics rows | ${metrics.length} (route × viewport) |`);
lines.push(`| Layout violations | ${layoutViolations.length} |`);
lines.push(
  "| CI | `qa-e2e.yml` non-blocking; §10.2 `qa-cross-platform.yml`; S2-Linux `linux-bundle.yml` (`agent-lounge-linux`) |",
);
lines.push("");
lines.push("## Verdict");
lines.push("");
lines.push(
  unexpectedFail === 0
    ? "Suite exit green for PR-0 harness (expected-fail cases intentionally failing until PR-1…5). L1–L6 hardened for live S2 findings (narrow panels, sparse interiors, New Node overlap)."
    : `Harness has ${unexpectedFail} unexpected failures — investigate before relying on CI signal.`,
);
lines.push("");
lines.push("## Per-test results");
lines.push("");
lines.push("| ID / title | Project | Status | Failure reason | Maps to |");
lines.push("|------------|---------|--------|----------------|---------|");
for (const r of rows) {
  let statusLabel = r.status;
  if (r.status === "expected" && r.expectedStatus === "passed") statusLabel = "pass";
  else if (r.status === "expected" && r.expectedStatus === "failed") statusLabel = "expected-fail";
  else if (r.status === "skipped") statusLabel = "skipped";
  else if (r.status === "unexpected") statusLabel = "FAIL";
  const reason = (r.error || "").replace(/\|/g, "\\|").slice(0, 140);
  lines.push(
    `| ${r.title.replace(/\|/g, "\\|")} | ${r.project} | ${statusLabel} | ${reason} | ${mapPr(r.title)} |`,
  );
}

lines.push("");
lines.push("## Layout metrics (L1–L6) per route × viewport");
lines.push("");
lines.push("| Route | Viewport | L1 ratio | L2 | L3 empty | L4 | L5 | L6 | Violations |");
lines.push("|-------|----------|----------|----|----------|----|----|----|------------|");
for (const m of metrics) {
  const v = [];
  if (!m.l1_pass) v.push(`L1=${(m.l1_widthRatio * 100).toFixed(0)}%`);
  if (!m.l2_pass) v.push("L2");
  if (!m.l3_pass) v.push(`L3=${(m.l3_largestEmptyRatio * 100).toFixed(0)}%`);
  if (!m.l4_pass) v.push("L4");
  if (!m.l5_pass) v.push("L5");
  if (m.l6_pass === false) v.push("L6");
  lines.push(
    `| ${m.route} | ${m.viewport.width}×${m.viewport.height} | ${(m.l1_widthRatio * 100).toFixed(1)}% | ${m.l2_pass ? "ok" : "fail"} | ${(m.l3_largestEmptyRatio * 100).toFixed(1)}% | ${m.l4_pass ? "ok" : "fail"} | ${m.l5_pass ? "ok" : "fail"} | ${m.l6_pass == null ? "n/a" : m.l6_pass ? "ok" : "fail"} | ${v.join(", ") || "—"} |`,
  );
}

lines.push("");
lines.push("## Failing blocker [B] cases (expected-fail baseline)");
lines.push("");
for (const b of blockerExpected) {
  lines.push(`- **${b.title}** (${b.project}): ${(b.error || "expected-fail").slice(0, 120)} → ${mapPr(b.title)}`);
}
if (!blockerExpected.length) lines.push("_None._");

lines.push("");
lines.push("## Untestable / plan corrections from PR-0");
lines.push("");
lines.push("- **GR-*** / **SH-06** / **DS-03** / **ST-04**: require live Tauri + OS dialogs / Graph child process (S2; Mac scripts Mac-specific; Win/Linux checklists under `scripts/qa/windows|linux/`).");
lines.push("- **AP-10**: native OS notification (Tauri plugin) on macOS/Windows/Linux — S2 live checklists (§10.2).");
lines.push("- **AP-03** `?demo=routing-banner` only works when `isTauri()===false`; harness uses `browser` fixture.");
lines.push("- **X-01** is TR/EN i18n (O1); **X-02** dotted-İ / `lang=tr` (live G2) — expected-fail until PR-2.");
lines.push("- **O2–O6** / §10.1–10.2: EX-05/EX-13/EX-14/EX-15, AP-06…10, SR-02, PATH-01, SH-04b — expected-fail until PR-2…5. **EX-14 + PATH-01 owned by PR-3 (UI wiring).**");
lines.push("- **FL-01/02** live NATS heartbeat needs real workers (S3); S1 page-render smoke + SR-02 default filter contract.");
lines.push("- **Empty fixture + `/onboarding`**: client error boundary under IPC mock — OB-01 uses full fixture + deselect.");
lines.push("- Deep-link for Mac route automation proposed in `scripts/qa/mac/` — **not** in product this PR (`data-qa` hooks only).");
lines.push("- Gemini’s `toHaveJSProperty('clientWidth', …)` is invalid in Playwright; L1–L6 use `getBoundingClientRect`.");
lines.push("- **§10.2 CI**: Playwright on Linux (`qa-e2e.yml`); Win/macOS `cargo test`+build (`qa-cross-platform.yml`); Linux AppImage/deb (`linux-bundle.yml`) — all non-blocking.");
lines.push("");
lines.push("## Live vs automated (S2 Mac 716f42c → harness)");
lines.push("");
lines.push("Source: [`docs/qa/live-baseline-2026-09-26-mac.md`](./live-baseline-2026-09-26-mac.md).");
lines.push("");
lines.push("| Live finding | Automated catch? | Case / metric |");
lines.push("|--------------|------------------|---------------|");
lines.push("| Single-panel Health/Fleet/Telemetry/Quotas/Settings ~30–63% width (G4) | Yes — L1 & L4 narrow single panel | `*-LAYOUT` |");
lines.push("| Panel frame full height, rows end ~15–30% | Yes — L2 sparse interior | `*-LAYOUT`, EX-LAYOUT |");
lines.push("| D3 /quotas horizontal overflow | Yes — L5 main/panel scrollWidth | `QUOTAS-LAYOUT` @ D3 |");
lines.push("| + New Node overlaps AL-OS CORE (G1) | Yes — L5 newNodeOverlap | SH-08b, `*-LAYOUT` L5 |");
lines.push("| `lang=tr` → TİME / SEMANTİC (G2) | Yes | **X-02** |");
lines.push("| Vault 400/800 vs bridge 2286/7958 | Yes — fixture mirrors mismatch | **EX-14** |");
lines.push("| Experience Log raw markdown + TR errors | Yes | **EX-15** |");
lines.push("| Stream 100% heartbeats + Decision: — | Yes — heartbeat events in fixture | **SR-02** |");
lines.push("| Long live path truncation / chip collisions | Partial — D2/D3 LAYOUT | S2 + LAYOUT |");
lines.push("| NSScreen menu-bar usable-height clamp | No (OS chrome) | S2 only |");
lines.push("");
lines.push(
  "Screenshots + Playwright JSON + layout-metrics: CI artifact `qa-baseline-<sha>` (`test-results/qa-baseline/`, not committed). Live report: `docs/qa/live-baseline-2026-09-26-mac.md`.",
);
lines.push("");

fs.writeFileSync(OUT_MD, lines.join("\n"));
console.log("Wrote", OUT_MD);
console.log({ pass, expectedFail, unexpectedFail, skipped, metrics: metrics.length, layoutViolations: layoutViolations.length });
