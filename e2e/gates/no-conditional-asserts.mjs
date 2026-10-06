#!/usr/bin/env node
/**
 * Fail if e2e specs reintroduce vacuous-pass conditionals:
 *   if (await locator.count()) { … expect … }
 *   waitForEvent(...).catch(() => null) around soft download asserts
 *
 * Lives under e2e/ (PR-C) so it does not touch scripts/qa/gates/* (PR-A).
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "../..");
const TESTS_DIR = path.join(ROOT, "e2e", "tests");

/** Soft existence guards that skip asserts when a locator is missing. */
const PATTERNS = [
  {
    id: "if-await-count",
    re: /if\s*\(\s*await\s+[^)]*\.count\(\s*\)\s*\)/,
    tip: "replace with await expect(locator).toBeVisible() (or toHaveCount) then act",
  },
  {
    id: "if-count-truthy",
    re: /if\s*\(\s*(?:await\s+)?[\w.[\]]+\.count\(\s*\)\s*\)/,
    tip: "do not gate asserts on .count(); require the control",
  },
  {
    id: "download-catch-null",
    re: /waitForEvent\(\s*["']download["'][^)]*\)\s*\.catch\(\s*\(\s*\)\s*=>\s*null\s*\)/,
    tip: "require the download (or IPC save) — do not swallow with .catch(() => null)",
  },
];

function listSpecs(dir) {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  for (const ent of fs.readdirSync(dir, { withFileTypes: true })) {
    if (ent.name.startsWith(".")) continue;
    const child = path.join(dir, ent.name);
    if (ent.isDirectory()) out.push(...listSpecs(child));
    else if (/\.(spec|test)\.(ts|tsx|js|jsx)$/.test(ent.name)) out.push(child);
  }
  return out.sort();
}

function scanFile(file) {
  const rel = path.relative(ROOT, file).replace(/\\/g, "/");
  const lines = fs.readFileSync(file, "utf8").split(/\r?\n/);
  const hits = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    // Allow documenting the ban itself in comments.
    if (/^\s*\/\//.test(line) || /^\s*\*/.test(line)) continue;
    for (const pat of PATTERNS) {
      if (pat.re.test(line)) {
        hits.push({ rel, line: i + 1, id: pat.id, tip: pat.tip, text: line.trim() });
      }
    }
  }
  return hits;
}

function selftest() {
  const probe = `
if (await node.count()) { await node.click(); }
if (await dl.count()) { /* soft */ }
await page.waitForEvent("download", { timeout: 5000 }).catch(() => null);
`;
  const tmpHits = [];
  for (const line of probe.split("\n")) {
    for (const pat of PATTERNS) {
      if (pat.re.test(line)) tmpHits.push(pat.id);
    }
  }
  const needed = new Set(PATTERNS.map((p) => p.id));
  for (const id of needed) {
    if (!tmpHits.includes(id)) {
      console.error(`FAIL: no-conditional-asserts self-test missed pattern '${id}'`);
      process.exit(1);
    }
  }
}

selftest();

const files = listSpecs(TESTS_DIR);
const allHits = files.flatMap(scanFile);

console.log("== e2e no-conditional-asserts ==");
if (allHits.length) {
  console.error(`FAIL: found ${allHits.length} conditional-guarded assertion pattern(s):`);
  for (const h of allHits) {
    console.error(`  ${h.rel}:${h.line} [${h.id}] ${h.text}`);
    console.error(`    → ${h.tip}`);
  }
  process.exit(1);
}
console.log("no-conditional-asserts: ok (0 soft count/download guards in e2e/tests)");
