#!/usr/bin/env node
/**
 * Build docs/qa/baseline-2026-09-26.md from Playwright JSON + layout-metrics.jsonl.
 */
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve("docs/qa/baseline-2026-09-26");
const REPORT_JSON = path.join(ROOT, "playwright-report.json");
const METRICS = path.join(ROOT, "layout-metrics.jsonl");
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
lines.push("| CI | `qa-e2e.yml` non-blocking (`continue-on-error`) |");
lines.push("");
lines.push("## Verdict");
lines.push("");
lines.push(
  unexpectedFail === 0
    ? "Suite exit green for PR-0 harness (expected-fail cases intentionally failing until PR-1…5)."
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
lines.push("- **GR-*** / **SH-06** / **DS-03** / **ST-04**: require live Tauri + OS dialogs / Graph child process (S2 Mac only).");
lines.push("- **AP-03** `?demo=routing-banner` only works when `isTauri()===false`; harness uses `browser` fixture.");
lines.push("- **X-01** is TR/EN i18n (O1 decided): dictionary + Settings switch + persistence; expected-fail until PR-2.");
lines.push("- **O2–O6** add EX-05/EX-13, archive-search S3, DS-03 Editor pref, AP-06/AP-07 destructive gate — harness cases expected-fail until PR-1…5.");
lines.push("- **FL-01/02** live NATS heartbeat needs real workers (S3); S1 only does page-render smoke.");
lines.push("- **Empty fixture + `/onboarding`**: client error boundary (“This page couldn't load”) under IPC mock — OB-01 uses full fixture + deselect; empty-onboarding still flaky.");
lines.push("- Deep-link for Mac route automation proposed in `scripts/qa/mac/` — **not** implemented in product this PR (`data-qa` hooks only).");
lines.push("- Gemini’s `toHaveJSProperty('clientWidth', …)` is invalid in Playwright; L1–L6 use `boundingBox` / `getBoundingClientRect` as planned.");
lines.push("");
lines.push("Screenshots: `docs/qa/baseline-2026-09-26/screenshots/` (JPEG). Playwright JSON: `playwright-report.json`.");
lines.push("");

fs.writeFileSync(OUT_MD, lines.join("\n"));
console.log("Wrote", OUT_MD);
console.log({ pass, expectedFail, unexpectedFail, skipped, metrics: metrics.length, layoutViolations: layoutViolations.length });
