#!/usr/bin/env node
/**
 * Fail if playwright.config.ts can silently shrink the e2e suite.
 *
 * 1) Static bans: top-level testMatch / testIgnore / grep / grepInvert;
 *    testDir must stay e2e/tests; project-level testDir / grep / grepInvert banned.
 * 2) Ground truth: `playwright test --list --reporter=json` unique files must
 *    equal the recursive on-disk spec set under e2e/tests (no fake regex on globs).
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "../../..");
const CONFIG_PATH = path.join(ROOT, "playwright.config.ts");
const EXPECTED_TEST_DIR_REL = path.join("e2e", "tests");
const TESTS_DIR = path.join(ROOT, EXPECTED_TEST_DIR_REL);
const SPEC_RE = /\.(test|spec)\.(ts|tsx|js|jsx|mjs)$/;

function normalizeRel(p) {
  return String(p).replace(/\\/g, "/").replace(/^\.\//, "");
}

/** Recursive on-disk specs relative to `dir` (posix separators). */
export function listDiskSpecsRecursive(dir) {
  const out = [];
  if (!fs.existsSync(dir)) return out;
  const walk = (abs, relBase) => {
    for (const ent of fs.readdirSync(abs, { withFileTypes: true })) {
      if (ent.name === "node_modules" || ent.name.startsWith(".")) continue;
      const rel = relBase ? `${relBase}/${ent.name}` : ent.name;
      const child = path.join(abs, ent.name);
      if (ent.isDirectory()) walk(child, rel);
      else if (ent.isFile() && SPEC_RE.test(ent.name)) out.push(normalizeRel(rel));
    }
  };
  walk(dir, "");
  return out.sort();
}

export function uniqueFilesFromListReport(report) {
  const files = new Set();
  const walk = (suites) => {
    if (!Array.isArray(suites)) return;
    for (const suite of suites) {
      for (const spec of suite.specs || []) {
        if (spec.file) files.add(normalizeRel(spec.file));
      }
      walk(suite.suites || []);
    }
  };
  walk(report.suites || []);
  return [...files].sort();
}

export function diffDiskVsListed(diskSpecs, listedFiles) {
  const disk = new Set(diskSpecs.map(normalizeRel));
  const listed = new Set(listedFiles.map(normalizeRel));
  const missing = [...disk].filter((f) => !listed.has(f)).sort();
  const extra = [...listed].filter((f) => !disk.has(f)).sort();
  return { missing, extra };
}

function normalizeTestDirValue(testDir, root = ROOT) {
  if (testDir == null) return null;
  const abs = path.isAbsolute(testDir)
    ? path.normalize(testDir)
    : path.normalize(path.join(root, testDir));
  return path.relative(root, abs).replace(/\\/g, "/") || ".";
}

/** Static rules on the defineConfig object (before --list). */
export function assertStaticConfigRules(config, { expectedTestDirRel = EXPECTED_TEST_DIR_REL } = {}) {
  const errors = [];
  for (const key of ["grep", "grepInvert", "testIgnore", "testMatch"]) {
    if (config[key] != null) {
      errors.push(`top-level ${key} is set — would silently shrink the suite`);
    }
  }
  const gotDir = normalizeTestDirValue(config.testDir);
  const expected = normalizeRel(expectedTestDirRel);
  if (gotDir == null) {
    errors.push(`testDir missing — must be ${expected}`);
  } else if (gotDir !== expected) {
    errors.push(`testDir is ${JSON.stringify(gotDir)} — must remain ${expected}`);
  }
  const projects = config.projects;
  if (!Array.isArray(projects) || projects.length === 0) {
    errors.push("config.projects must be a non-empty array");
    return errors;
  }
  for (const p of projects) {
    const name = JSON.stringify(p.name ?? "?");
    if (p.testDir != null) {
      errors.push(`project ${name} sets testDir — forbidden (silent relocation)`);
    }
    if (p.grep != null || p.grepInvert != null) {
      errors.push(`project ${name} sets grep/grepInvert — forbidden`);
    }
  }
  return errors;
}

export function assertListCoversDisk(diskSpecs, listedFiles) {
  const { missing, extra } = diffDiskVsListed(diskSpecs, listedFiles);
  const errors = [];
  if (missing.length) {
    errors.push(
      `playwright --list omitted ${missing.length} on-disk spec(s): ${missing.join(", ")}`,
    );
  }
  if (extra.length) {
    errors.push(
      `playwright --list reported unexpected file(s) outside disk scan: ${extra.join(", ")}`,
    );
  }
  return errors;
}

function runPlaywrightList(configPath, cwd = ROOT) {
  const pwBin = path.join(ROOT, "node_modules", "@playwright", "test", "cli.js");
  const alt = path.join(ROOT, "node_modules", "playwright", "cli.js");
  const bin = fs.existsSync(pwBin) ? pwBin : alt;
  if (!fs.existsSync(bin)) {
    throw new Error("playwright CLI not found — run npm ci first");
  }
  const r = spawnSync(
    process.execPath,
    [bin, "test", "--list", "--reporter=json", `--config=${configPath}`],
    {
      cwd,
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
      env: { ...process.env, CI: process.env.CI || "" },
    },
  );
  if (r.error) throw r.error;
  const stdout = r.stdout || "";
  // JSON reporter prints a single object; tolerate leading noise.
  const start = stdout.indexOf("{");
  if (start < 0) {
    throw new Error(
      `playwright --list produced no JSON (status=${r.status}):\n${r.stderr || stdout}`,
    );
  }
  let report;
  try {
    report = JSON.parse(stdout.slice(start));
  } catch (err) {
    throw new Error(`failed to parse playwright --list JSON: ${err}\n${stdout.slice(0, 500)}`);
  }
  if (r.status !== 0 && !(report.suites && report.suites.length)) {
    throw new Error(
      `playwright --list failed (status=${r.status}):\n${r.stderr || stdout.slice(0, 800)}`,
    );
  }
  return report;
}

function writeTempSelftestProject() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pw-cfg-cov-"));
  const tests = path.join(dir, "tests");
  fs.mkdirSync(tests, { recursive: true });
  fs.mkdirSync(path.join(tests, "nested"), { recursive: true });
  // Playwright resolves `@playwright/test` from the config directory — link
  // the repo's node_modules so hostile temp configs can load.
  const nm = path.join(ROOT, "node_modules");
  if (!fs.existsSync(nm)) {
    throw new Error("self-test needs node_modules — run npm ci first");
  }
  fs.symlinkSync(nm, path.join(dir, "node_modules"), "dir");
  const body = (title) =>
    `import { test } from '@playwright/test';\ntest(${JSON.stringify(title)}, async () => {});\n`;
  fs.writeFileSync(path.join(tests, "kept.spec.ts"), body("kept"));
  fs.writeFileSync(path.join(tests, "dropped.spec.ts"), body("dropped"));
  fs.writeFileSync(path.join(tests, "nested", "deep.spec.ts"), body("deep"));
  return dir;
}

function selfTest() {
  // --- static rules ---
  const goodStatic = {
    testDir: "./e2e/tests",
    projects: [{ name: "D0" }, { name: "D1", testIgnore: /hydration/ }],
  };
  if (assertStaticConfigRules(goodStatic).length !== 0) {
    throw new Error("self-test: good static config must pass");
  }
  if (
    assertStaticConfigRules({
      testDir: "./e2e/tests",
      testMatch: /vault-dashboard/,
      projects: [{ name: "D0" }],
    }).length === 0
  ) {
    throw new Error("self-test: top-level testMatch must FAIL");
  }
  if (
    assertStaticConfigRules({
      testDir: "./e2e/tests/nested",
      projects: [{ name: "D0" }],
    }).length === 0
  ) {
    throw new Error("self-test: relocated testDir must FAIL");
  }
  if (
    assertStaticConfigRules({
      testDir: "./e2e/tests",
      projects: [{ name: "X", testDir: "./other" }],
    }).length === 0
  ) {
    throw new Error("self-test: project testDir must FAIL");
  }
  if (
    assertStaticConfigRules({
      testDir: "./e2e/tests",
      projects: [{ name: "X", grep: /x/ }],
    }).length === 0
  ) {
    throw new Error("self-test: project grep must FAIL");
  }

  // --- list vs disk (pure) ---
  const disk = ["a.spec.ts", "nested/b.spec.ts", "c.spec.ts"];
  const listedOk = ["a.spec.ts", "nested/b.spec.ts", "c.spec.ts"];
  if (assertListCoversDisk(disk, listedOk).length !== 0) {
    throw new Error("self-test: matching list/disk must pass");
  }
  const miss = assertListCoversDisk(disk, ["a.spec.ts", "c.spec.ts"]);
  if (!miss.some((e) => e.includes("nested/b.spec.ts"))) {
    throw new Error(`self-test: missing nested spec must FAIL, got: ${miss.join("; ")}`);
  }

  // --- integration: real playwright --list with hostile configs ---
  const tmp = writeTempSelftestProject();
  try {
    const matchCfg = path.join(tmp, "match.config.ts");
    fs.writeFileSync(
      matchCfg,
      `
import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: ${JSON.stringify(path.join(tmp, "tests"))},
  testMatch: /kept/,
  projects: [{ name: 'P0' }],
});
`,
    );
    const matchReport = runPlaywrightList(matchCfg, ROOT);
    const matchListed = uniqueFilesFromListReport(matchReport);
    const matchDisk = listDiskSpecsRecursive(path.join(tmp, "tests"));
    const matchErrs = assertListCoversDisk(matchDisk, matchListed);
    if (matchErrs.length === 0) {
      throw new Error(
        `self-test: top-level testMatch /kept/ must drop specs via --list; listed=${matchListed}`,
      );
    }

    const subdir = path.join(tmp, "tests", "nested");
    const dirCfg = path.join(tmp, "dir.config.ts");
    fs.writeFileSync(
      dirCfg,
      `
import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: ${JSON.stringify(subdir)},
  projects: [{ name: 'P0' }],
});
`,
    );
    const dirReport = runPlaywrightList(dirCfg, ROOT);
    const dirListed = uniqueFilesFromListReport(dirReport);
    // Compare against the full on-disk tree (parent), not the narrowed testDir.
    const fullDisk = listDiskSpecsRecursive(path.join(tmp, "tests"));
    const dirErrs = assertListCoversDisk(fullDisk, dirListed);
    if (dirErrs.length === 0) {
      throw new Error(
        `self-test: narrowed testDir must FAIL vs full disk; listed=${dirListed}`,
      );
    }
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }

  console.log("playwright-config-coverage: self-test ok");
}

async function main() {
  selfTest();

  if (!fs.existsSync(CONFIG_PATH)) {
    console.error(`FAIL: missing ${CONFIG_PATH}`);
    process.exit(1);
  }

  const mod = await import(pathToFileURL(CONFIG_PATH).href);
  const config = mod.default ?? mod;
  if (!config || !Array.isArray(config.projects)) {
    console.error("FAIL: playwright.config.ts did not export defineConfig with projects");
    process.exit(1);
  }

  const staticErrs = assertStaticConfigRules(config);
  const diskSpecs = listDiskSpecsRecursive(TESTS_DIR);
  if (diskSpecs.length === 0) {
    console.error(`FAIL: no specs under ${TESTS_DIR}`);
    process.exit(1);
  }

  console.log("== playwright-config-coverage ==");
  console.log(`diskSpecs=${diskSpecs.length} (recursive under ${EXPECTED_TEST_DIR_REL})`);

  let listErrs = [];
  try {
    const report = runPlaywrightList(CONFIG_PATH, ROOT);
    const listed = uniqueFilesFromListReport(report);
    const stats = report.stats || {};
    console.log(
      `listFiles=${listed.length} listTests=${stats.total ?? "?"} projects=${config.projects.length}`,
    );
    listErrs = assertListCoversDisk(diskSpecs, listed);
  } catch (err) {
    listErrs = [`playwright --list failed: ${err?.message || err}`];
  }

  const errors = [...staticErrs, ...listErrs];
  if (errors.length) {
    console.error("FAIL: playwright config can silently exclude tests:");
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }
  console.log(
    "playwright-config-coverage: ok (static pins + --list covers every on-disk spec)",
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
