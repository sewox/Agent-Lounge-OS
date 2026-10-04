import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { generateReport } from "../scripts/report.mjs";
import { parseDelays, runMatrix, DEFAULT_DELAYS_S } from "../scripts/run-matrix.mjs";
import { ANTIGRAVITY_CLIFF_DELAYS_S } from "../scripts/run-antigravity-cliff.mjs";

test("antigravity cliff delay list is 150/170/180/190 (manual script only)", () => {
  assert.deepEqual(ANTIGRAVITY_CLIFF_DELAYS_S, [150, 170, 180, 190]);
});

test("parseDelays defaults only when unset", () => {
  assert.deepEqual(parseDelays(undefined), DEFAULT_DELAYS_S);
  assert.deepEqual(parseDelays(null), DEFAULT_DELAYS_S);
  assert.deepEqual(parseDelays("5,15,25"), [5, 15, 25]);
  assert.deepEqual(parseDelays("0.05 0.1"), [0.05, 0.1]);
});

test("parseDelays rejects empty, non-numeric, negative", () => {
  assert.throws(() => parseDelays(""), /empty/);
  assert.throws(() => parseDelays("   "), /empty/);
  assert.throws(() => parseDelays("abc"), /invalid delay/);
  assert.throws(() => parseDelays("5,abc,10"), /invalid delay/);
  assert.throws(() => parseDelays("-1"), /invalid delay/);
  assert.throws(() => parseDelays("5,-0.1"), /invalid delay/);
});

test("runMatrix short delays produce JSON + markdown report", async () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-matrix-"));
  const { result, matrixPath, reportPath } = await runMatrix({
    delaysS: [0.05, 0.1],
    outDir,
    withProgress: true,
  });

  assert.equal(result.rows.length, 4); // 2 delays × progress on/off
  assert.ok(result.rows.every((r) => r.status === "ok"));
  assert.ok(fs.existsSync(matrixPath));
  assert.ok(fs.existsSync(reportPath));

  const md = fs.readFileSync(reportPath, "utf8");
  assert.match(md, /Run matrix/);
  assert.match(md, /0\.05/);
  assert.match(md, /Session summaries/);
});

test("runMatrix refuses zero-length delay list", async () => {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-matrix-z-"));
  await assert.rejects(
    () => runMatrix({ delaysS: [], outDir }),
    /zero delay rows/,
  );
});

test("generateReport renders timeout column from fixture", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mcp-probe-report-"));
  const matrixPath = path.join(dir, "matrix-results.json");
  fs.writeFileSync(
    matrixPath,
    JSON.stringify({
      meta: { harness: "test" },
      rows: [
        {
          delay_s: 60,
          progress: true,
          status: "client_timeout",
          duration_ms: 45000,
          timeout_s: 45,
          progress_token_present: true,
          progress_notifications: 4,
          cancelled: true,
          log_file: "x.jsonl",
        },
      ],
    }),
    "utf8",
  );
  const md = generateReport({ matrixPath, outPath: path.join(dir, "r.md") });
  assert.match(md, /client_timeout/);
  assert.match(md, /45/);
  assert.match(md, /yes/);
});
