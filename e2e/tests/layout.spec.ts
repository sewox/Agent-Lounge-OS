import { test, expect } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { assertLayoutL1toL6, type LayoutMetrics } from "../helpers/layout";
import { openRoute, ROUTES } from "../helpers/nav";

/** Ignored output — uploaded as CI artifact `qa-baseline-*`, not committed under docs/. */
const METRICS_DIR = path.join("test-results", "qa-baseline");
const SHOT_DIR = path.join(METRICS_DIR, "screenshots");

function appendMetric(m: LayoutMetrics) {
  fs.mkdirSync(METRICS_DIR, { recursive: true });
  const file = path.join(METRICS_DIR, "layout-metrics.jsonl");
  fs.appendFileSync(file, `${JSON.stringify(m)}\n`);
}

test.describe("*-LAYOUT L1–L6 every route × viewport", () => {
  for (const route of ROUTES) {
    test(`${route.id.toUpperCase()}-LAYOUT · ${route.path}`, async ({ page }, testInfo) => {
      testInfo.annotations.push({ type: "id", description: `${route.id.toUpperCase()}-LAYOUT` });
      await openRoute(page, route.path, "full");
      const metrics = await assertLayoutL1toL6(page, route.path, expect);
      appendMetric(metrics);

      fs.mkdirSync(SHOT_DIR, { recursive: true });
      const shotName = `${route.id}__${testInfo.project.name}__${metrics.viewport.width}x${metrics.viewport.height}.jpg`;
      await page.screenshot({
        path: path.join(SHOT_DIR, shotName),
        type: "jpeg",
        quality: 62,
        fullPage: false,
      });
    });
  }
});
