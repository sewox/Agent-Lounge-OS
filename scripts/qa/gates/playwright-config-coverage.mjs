#!/usr/bin/env node
/**
 * Fail if playwright.config.ts can silently exclude tests via top-level
 * grep / grepInvert / testIgnore, or if any e2e spec is unmatched by every project.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "../../..");
const CONFIG_PATH = path.join(ROOT, "playwright.config.ts");
const TESTS_DIR = path.join(ROOT, "e2e", "tests");

function asRegexList(value) {
  if (value == null) return [];
  const list = Array.isArray(value) ? value : [value];
  return list.map((v) => (v instanceof RegExp ? v : new RegExp(v)));
}

/** Whether a project would pick up `relPath` (basename or relative under testDir). */
function projectAcceptsFile(project, relPath) {
  const base = path.basename(relPath);
  const haystacks = [relPath, base, relPath.replace(/\\/g, "/")];
  const ignores = asRegexList(project.testIgnore);
  if (ignores.some((re) => haystacks.some((h) => re.test(h)))) {
    return false;
  }
  const matches = asRegexList(project.testMatch);
  if (matches.length === 0) {
    return /\.(test|spec)\.(js|ts|mjs|jsx|tsx)$/.test(base);
  }
  return matches.some((re) => haystacks.some((h) => re.test(h)));
}

function assertConfigCannotSilentlyExclude(config, specRelPaths) {
  const errors = [];
  for (const key of ["grep", "grepInvert", "testIgnore"]) {
    if (config[key] != null) {
      errors.push(
        `top-level ${key} is set — would filter the whole suite silently`,
      );
    }
  }
  const projects = config.projects;
  if (!Array.isArray(projects) || projects.length === 0) {
    errors.push("config.projects must be a non-empty array");
    return errors;
  }
  const fullCoverage = projects.filter(
    (p) =>
      p.testIgnore == null &&
      p.testMatch == null &&
      p.grep == null &&
      p.grepInvert == null,
  );
  if (fullCoverage.length === 0) {
    errors.push(
      "no full-coverage project (without testIgnore/testMatch/grep/grepInvert)",
    );
  }
  for (const p of projects) {
    if (p.grep != null || p.grepInvert != null) {
      errors.push(
        `project ${JSON.stringify(p.name ?? "?")} sets grep/grepInvert — forbidden`,
      );
    }
  }
  for (const rel of specRelPaths) {
    if (!projects.some((p) => projectAcceptsFile(p, rel))) {
      errors.push(`spec not accepted by any project: ${rel}`);
    }
  }
  return errors;
}

function listSpecRelPaths(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs
    .readdirSync(dir)
    .filter((f) => /\.(test|spec)\.(ts|tsx|js|jsx|mjs)$/.test(f));
}

function selfTest() {
  const okSpecs = ["hydration.spec.ts", "vault-dashboard.spec.ts"];
  const good = {
    projects: [{ name: "D0" }, { name: "D1", testIgnore: /hydration/ }],
  };
  const goodErrs = assertConfigCannotSilentlyExclude(good, okSpecs);
  if (goodErrs.length !== 0) {
    throw new Error(`self-test expected good config to pass:\n${goodErrs.join("\n")}`);
  }
  if (
    assertConfigCannotSilentlyExclude(
      { grep: /only-this/, projects: [{ name: "D0" }] },
      okSpecs,
    ).length === 0
  ) {
    throw new Error("self-test: top-level grep must fail");
  }
  if (
    assertConfigCannotSilentlyExclude(
      { testIgnore: /./, projects: [{ name: "D0" }] },
      okSpecs,
    ).length === 0
  ) {
    throw new Error("self-test: top-level testIgnore must fail");
  }
  if (
    assertConfigCannotSilentlyExclude(
      {
        projects: [
          { name: "A", testIgnore: /hydration/ },
          { name: "B", testMatch: /vault/ },
        ],
      },
      okSpecs,
    ).length === 0
  ) {
    throw new Error("self-test: missing full-coverage project must fail");
  }
  if (
    assertConfigCannotSilentlyExclude(
      { projects: [{ name: "D0" }, { name: "X", grep: /x/ }] },
      okSpecs,
    ).length === 0
  ) {
    throw new Error("self-test: project grep must fail");
  }
  const uErrs = assertConfigCannotSilentlyExclude(
    {
      projects: [
        { name: "D0", testIgnore: /hydration/ },
        { name: "D1", testIgnore: /hydration/ },
      ],
    },
    okSpecs,
  );
  if (!uErrs.some((e) => e.includes("hydration.spec.ts"))) {
    throw new Error(`self-test: expected uncovered hydration, got:\n${uErrs.join("\n")}`);
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

  const specs = listSpecRelPaths(TESTS_DIR);
  if (specs.length === 0) {
    console.error(`FAIL: no specs under ${TESTS_DIR}`);
    process.exit(1);
  }

  const errors = assertConfigCannotSilentlyExclude(config, specs);
  console.log("== playwright-config-coverage ==");
  console.log(`specs=${specs.length} projects=${config.projects.length}`);
  if (errors.length) {
    console.error("FAIL: playwright config can silently exclude tests:");
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }
  console.log(
    "playwright-config-coverage: ok (full-coverage project + every spec matched)",
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
