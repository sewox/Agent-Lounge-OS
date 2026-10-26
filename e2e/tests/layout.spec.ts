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
      // Landing view — same as main (no pre-measure drill-down / drawer open).
      const metrics = await assertLayoutL1toL6(page, route.path, expect);
      appendMetric(metrics);

      // Optional extra: vault map after explicit project open still holds L1–L6.
      if (route.path === "/vault") {
        const back = page.locator('[data-qa="vault-back-projects"]');
        if (await back.isVisible()) {
          await back.click();
          await expect(page.locator('[data-qa="vault-semantic-map"]')).toHaveAttribute(
            "data-view",
            "projects",
          );
        }
        const project = page
          .locator('[data-qa="vault-project-row"]')
          .filter({ hasText: /Agent-Lounge-OS/i })
          .first();
        await expect(project).toBeVisible();
        await project.locator('[data-qa="vault-project-open"]').click();
        await expect(page.locator('[data-qa="vault-semantic-map"]')).toHaveAttribute(
          "data-view",
          "pages",
        );
        const inspectorInline = page.locator('[data-panel-slot="inspector"]');
        if (!(await inspectorInline.isVisible())) {
          const closeBtn = page.locator('[data-qa="close-panel-drawer"]');
          if (await closeBtn.isVisible()) {
            await closeBtn.click();
          }
          await page.locator('[data-qa="open-inspector-drawer"]').click();
          await expect(page.locator('[data-qa="panel-grid-drawer-inspector"]')).toBeVisible();
        }
        const detailInline = page.locator('[data-panel-slot="detail"]');
        if (!(await detailInline.isVisible())) {
          const closeBtn = page.locator('[data-qa="close-panel-drawer"]');
          if (await closeBtn.isVisible()) {
            await closeBtn.click();
            await expect(page.locator('[data-qa="panel-grid-drawer"]')).toHaveCount(0);
          }
          await page.locator('[data-qa="open-detail-drawer"]').click();
          await expect(page.locator('[data-qa="panel-grid-drawer-detail"]')).toBeVisible();
        }
        await expect(page.locator('[data-qa="vault-page-row"]').first()).toBeVisible();
        const afterDrill = await assertLayoutL1toL6(page, route.path, expect);
        appendMetric(afterDrill);
      }

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
