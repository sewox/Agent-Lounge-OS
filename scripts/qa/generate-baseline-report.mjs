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
  const id = (title.match(/^[A-Z0-9*-]+/) || [title])[0];
  for (const row of PR_MAP) {
    if (row.re.test(id) || row.re.test(title)) return row.pr;
  }
  if (/LAYOUT/i.test(title)) return "PR-2 Cleanup-Shell";
  return "—";
}

function loadSuites(suite, acc = []) {
  if (!suite) return acc;
  if (suite.specs) {
    for (const spec of suite.specs) {
      for (const t of spec.tests || []) {
        for (const r of t.results || []) {
          acc.push({
            title: spec.title,
            project: t.projectName,
            status: r.status,
            error: r.error?.message?.split("\n")[0] || "",
            annotations: (spec.tags || []).concat(
              (t.annotations || []).map((a) => `${a.type}:${a.description || ""}`),
            ),
            expected: (t.annotations || []).some((a) => a.type === "expected-fail")
              || r.status === "expected"
              || (spec.title || "").includes("expected-fail"),
          });
        }
      }
    }
  }
  for (const child of suite.suites || []) loadSuites(child, acc);
  return acc;
}

const json = JSON.parse(fs.readFileSync(REPORT_JSON, "utf8"));
const rows = [];
for (const suite of json.suites || []) loadSuites(suite, rows);

let pass = 0;
let fail = 0;
let expectedFail = 0;
let skipped = 0;

for (const r of rows) {
  if (r.status === "skipped") {
    skipped++;
    continue;
  }
  if (r.status === "expected" || (r.status === "failed" && r.expected)) {
    // Playwright: "unexpected" = failed, "expected" = test.fail() that failed as expected
  }
  if (r.status === "passed") pass++;
  else if (r.status === "expected") expectedFail++;
  else if (r.status === "failed" || r.status === "unexpected" || r.status === "timedOut") {
    if ((r.title || "").includes("expected-fail") || r.annotations.some((a) => String(a).startsWith("expected-fail"))) {
      expectedFail++;
    } else {
      fail++;
    }
  } else if (r.status === "flaky") pass++;
  else fail++;
}

const metrics = fs.existsSync(METRICS)
  ? fs.readFileSync(METRICS, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l))
  : [];

const blockers = rows.filter((r) => {
  const isB = /\[B\]|^SH-0[128]|^DB-0[124]|^EX-|^DS-0[125]|^HM-0[1235]|^ST-01|^X-0[34]|LAYOUT/i.test(r.title);
  const failed = r.status === "failed" || r.status === "unexpected" || r.status === "expected";
  return isB && failed;
});

const lines = [];
lines.push("# QA Baseline — 2026-09-26 (PR-0)");
lines.push("");
lines.push("| Field | Value |");
lines.push("|-------|-------|");
lines.push(`| Commit base | \`15e19e9\` (main) |`);
lines.push(`| Generated | ${new Date().toISOString()} |`);
lines.push(`| Suite | Playwright e2e + S4 gates |`);
lines.push(`| Counts | pass=${pass} · fail=${fail} · expected-fail=${expectedFail} · skipped=${skipped} · total_rows=${rows.length} |`);
lines.push("");
lines.push("## Per-test results");
lines.push("");
lines.push("| ID / title | Project | Status | Failure reason | Maps to |");
lines.push("|------------|---------|--------|----------------|---------|");
for (const r of rows) {
  const status =
    r.status === "expected"
      ? "expected-fail"
      : r.status === "passed"
        ? "pass"
        : r.status;
  const reason = (r.error || "").replace(/\|/g, "\\|").slice(0, 160);
  lines.push(
    `| ${r.title.replace(/\|/g, "\\|")} | ${r.project} | ${status} | ${reason} | ${mapPr(r.title)} |`,
  );
}

lines.push("");
lines.push("## Layout metrics (L1–L6) per route × viewport");
lines.push("");
lines.push("| Route | Viewport | L1 ratio | L2 | L3 empty | L4 narrow | L5 | L6 | Violations |");
lines.push("|-------|----------|----------|----|----------|-----------|----|----|------------|");
for (const m of metrics) {
  const v = [];
  if (!m.l1_pass) v.push("L1");
  if (!m.l2_pass) v.push("L2");
  if (!m.l3_pass) v.push("L3");
  if (!m.l4_pass) v.push("L4");
  if (!m.l5_pass) v.push("L5");
  if (m.l6_pass === false) v.push("L6");
  lines.push(
    `| ${m.route} | ${m.viewport.width}×${m.viewport.height} | ${(m.l1_widthRatio * 100).toFixed(1)}% | ${m.l2_pass ? "ok" : "fail"} | ${(m.l3_largestEmptyRatio * 100).toFixed(1)}% | ${m.l4_pass ? "ok" : "fail"} | ${m.l5_pass ? "ok" : "fail"} | ${m.l6_pass == null ? "n/a" : m.l6_pass ? "ok" : "fail"} | ${v.join(",") || "—"} |`,
  );
}

lines.push("");
lines.push("## Failing / expected-fail [B] blockers (baseline)");
lines.push("");
for (const b of blockers) {
  lines.push(`- **${b.title}** (${b.project}): ${b.error || b.status} → ${mapPr(b.title)}`);
}
if (!blockers.length) lines.push("_None parsed — see table above._");

lines.push("");
lines.push("## Untestable / plan corrections from PR-0");
lines.push("");
lines.push("- **GR-*** Graph UI lifecycle and **SH-06** Index folder picker require real Tauri + OS dialogs (S2 Mac only).");
lines.push("- **AP-03** `?demo=routing-banner` only works when `isTauri()===false`; Tauri IPC mock disables the demo path — test uses `browser` fixture.");
lines.push("- **X-01** language unity blocked on open decision **O1** (mixed TR/EN documented, not hard-fail).");
lines.push("- **FL-01/02** live heartbeat needs real NATS workers (S3); page render smoke only in S1.");
lines.push("- Deep-link for Mac route automation proposed in `scripts/qa/mac/` — **not** implemented in product this PR.");
lines.push("");
lines.push("Screenshots: `docs/qa/baseline-2026-09-26/screenshots/` (JPEG).");
lines.push("");

fs.writeFileSync(OUT_MD, lines.join("\n"));
console.log("Wrote", OUT_MD);
console.log({ pass, fail, expectedFail, skipped, metrics: metrics.length });
