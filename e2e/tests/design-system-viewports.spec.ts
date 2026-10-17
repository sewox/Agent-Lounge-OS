import { test, expect } from "@playwright/test";
import { assertLayoutL1toL6 } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

const DS_LAB = "/ds-lab";

/**
 * PR-UI-Tuning viewport matrix (runs under D0 only — see playwright.config.ts
 * testIgnore on D1/D2/D3). Exact panel mode from container width
 * (data-panel-width) plus L1–L6 at each size. No artificial max-width at 2560.
 */
const VIEWPORT_MODE_CASES = [
  { width: 1366, height: 768, mode: "one" as const },
  { width: 1600, height: 900, mode: "two" as const },
  { width: 1920, height: 1080, mode: "three" as const },
  { width: 2560, height: 1440, mode: "three" as const },
] as const;

test.describe("PR-UI-Tuning ds-lab viewport matrix", () => {
  for (const vp of VIEWPORT_MODE_CASES) {
    test(`DS-VP · ${vp.width}×${vp.height} → ${vp.mode} + L1–L6`, async ({ page }) => {
      test.info().annotations.push({
        type: "id",
        description: `DS-VP-${vp.width}x${vp.height}`,
      });
      await page.setViewportSize({ width: vp.width, height: vp.height });
      await openRoute(page, DS_LAB, "full");
      await expect(page.locator('[data-qa="ds-lab"]')).toBeVisible();

      const grid = page.locator('[data-qa="panel-grid"]');
      await expect(grid).toBeVisible();
      await expect(grid).toHaveAttribute("data-panel-mode", vp.mode);

      const widthAttr = await grid.getAttribute("data-panel-width");
      expect(widthAttr).toBeTruthy();
      const panelWidth = Number(widthAttr);
      expect(Number.isFinite(panelWidth)).toBe(true);
      expect(panelWidth).toBeGreaterThan(0);

      if (vp.mode === "one") {
        expect(panelWidth).toBeLessThan(1200);
      } else if (vp.mode === "two") {
        expect(panelWidth).toBeGreaterThanOrEqual(1200);
        expect(panelWidth).toBeLessThan(1600);
      } else {
        expect(panelWidth).toBeGreaterThanOrEqual(1600);
      }

      if (vp.mode === "three") {
        await expect(page.locator('[data-panel-slot="inspector"]')).toBeVisible();
        await expect(page.locator('[data-qa="open-inspector-drawer"]')).toHaveCount(0);
      } else if (vp.mode === "two") {
        await expect(page.locator('[data-panel-slot="detail"]')).toBeVisible();
        await expect(page.locator('[data-panel-slot="inspector"]')).toHaveCount(0);
      } else {
        await expect(page.locator('[data-qa="open-detail-drawer"]')).toBeVisible();
      }

      // 2560: panels widen in proportion — no max-width clamp on the grid.
      if (vp.width === 2560) {
        expect(panelWidth / vp.width).toBeGreaterThanOrEqual(0.8);
        const box = await grid.boundingBox();
        expect(box).toBeTruthy();
        expect(box!.width / vp.width).toBeGreaterThanOrEqual(0.8);
        // Grid fills the content column (sidebar excluded); no artificial cap.
        expect(Math.abs(box!.width - panelWidth)).toBeLessThanOrEqual(2);
      }

      const metrics = await assertLayoutL1toL6(page, DS_LAB, expect);
      // L2 sparse-interior must pass on natural content depth (no bottom-pinned sentinels).
      expect(metrics.l2_sparseInterior).toBe(false);
      expect(metrics.l2_minInteriorFill).toBeGreaterThanOrEqual(0.45);
    });
  }
});
