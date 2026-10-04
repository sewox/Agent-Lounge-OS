#!/usr/bin/env node
/**
 * Antigravity cliff matrix — MANUAL ONLY (not in CI / npm test).
 *
 * Verifies the 120–180s window around the measured 180s hard client timeout:
 * delays 150, 170, 180, 190 seconds (plain + progress variants).
 *
 * Expected (from 2026-10-04 measurement):
 * - progress does NOT extend the timeout
 * - hard cut at ~180.0s with notifications/cancelled reason "deadline exceeded"
 * - connection stays open
 *
 * Usage:
 *   node scripts/run-antigravity-cliff.mjs
 *   node scripts/run-antigravity-cliff.mjs --out-dir ./logs/antigravity-cliff
 *
 * Wall time ≈ (150+170+180+190)*2 ≈ 23 minutes if all complete/timeout.
 */
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runMatrix } from "./run-matrix.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** @type {number[]} */
export const ANTIGRAVITY_CLIFF_DELAYS_S = [150, 170, 180, 190];

const isMain =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMain) {
  const outArg = process.argv.indexOf("--out-dir");
  const outDir =
    outArg >= 0 && process.argv[outArg + 1]
      ? path.resolve(process.argv[outArg + 1])
      : path.resolve(__dirname, "../logs/antigravity-cliff");

  process.stderr.write(
    `[mcp-probe] Antigravity cliff matrix (MANUAL): delays=${ANTIGRAVITY_CLIFF_DELAYS_S.join(",")}s → ${outDir}\n`,
  );
  process.stderr.write(
    "[mcp-probe] This is intentionally slow (~20+ min). Not part of CI.\n",
  );

  try {
    const { result, matrixPath, reportPath } = await runMatrix({
      delaysS: ANTIGRAVITY_CLIFF_DELAYS_S,
      outDir,
      withProgress: true,
      // Probe-side abort slightly above 180s hard limit so we observe server/client cancel.
      clientTimeoutMs: 195_000,
    });
    process.stderr.write(
      `[mcp-probe] cliff rows=${result.rows.length} json=${matrixPath} report=${reportPath}\n`,
    );
    // Summarize for humans
    for (const row of result.rows) {
      process.stderr.write(
        `  delay=${row.delay_s}s progress=${row.progress} status=${row.status} duration_ms=${row.duration_ms} cancelled=${row.cancelled}\n`,
      );
    }
  } catch (err) {
    process.stderr.write(
      `[mcp-probe] ${err instanceof Error ? err.message : String(err)}\n`,
    );
    process.exit(1);
  }
}
