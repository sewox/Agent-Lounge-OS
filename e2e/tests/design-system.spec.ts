import { test, expect, type Page } from "@playwright/test";
import { assertLayoutL1toL6, LAYOUT_VIEWPORTS } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

const DS_LAB = "/ds-lab";

/** Detail is inline (two/three/portrait) or behind the narrow-mode drawer trigger. */
async function revealDetailPanel(page: Page) {
  const inline = page.locator('[data-panel-slot="detail"]');
  const drawer = page.locator('[data-qa="panel-grid-drawer-detail"]');
  const openBtn = page.locator('[data-qa="open-detail-drawer"]');
  const inlineVisible = await inline.isVisible();
  if (!inlineVisible) {
    await expect(openBtn).toBeVisible();
    await openBtn.click();
  }
  await expect(inline.or(drawer)).toBeVisible();
}

test.describe("PR-DS-0 design-system reference", () => {
  test("DS-01 · StatsStrip click sets filter + aria-pressed", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "DS-01" });
    await openRoute(page, DS_LAB, "full");

    const strip = page.locator('[data-qa="stats-strip"]');
    await expect(strip).toBeVisible();

    const halted = page.locator('[data-qa="stats-strip-item"][data-stat-id="halted"]');
    await expect(halted).toBeVisible();
    await halted.click();
    await expect(halted).toHaveAttribute("aria-pressed", "true");
    await expect(halted).toHaveAttribute("data-active", "true");

    // List shows only halted rows after filter.
    const rows = page.locator('[data-qa="ds-lab-row"]');
    await expect(rows.first()).toBeVisible();
    const count = await rows.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      await expect(rows.nth(i).locator('[data-qa="status-pill"]')).toHaveAttribute(
        "data-tone",
        "halted",
      );
    }

    await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
    await expect(page.locator('[data-qa="halted-hatch"]')).toBeVisible();
  });

  test("DS-02 · Halted pill distinct from error pill", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "DS-02" });
    await openRoute(page, DS_LAB, "full");

    await page.locator('[data-qa="stats-strip-item"][data-stat-id="all"]').click();
    await page.locator('[data-qa="ds-lab-row"][data-row-id="a4"]').click();
    await expect(
      page.locator('[data-qa="ds-lab-row"][data-row-id="a4"] [data-qa="status-pill"]'),
    ).toHaveAttribute("data-tone", "error");
    await expect(page.locator('[data-qa="halted-banner"]')).toHaveCount(0);
    await expect(page.locator('[data-qa="ds-lab-halted-idle"]')).toBeVisible();

    await revealDetailPanel(page);
    const detailPill = page.locator(
      '[data-panel-slot="detail"] [data-qa="status-pill"], [data-qa="panel-grid-drawer-detail"] [data-qa="status-pill"]',
    );
    await expect(detailPill.first()).toHaveAttribute("data-tone", "error");

    // Close drawer if open so the next selection starts clean on narrow viewports.
    const closeBtn = page.locator('[data-qa="close-panel-drawer"]');
    if (await closeBtn.isVisible()) {
      await closeBtn.click();
    }

    await page.locator('[data-qa="ds-lab-row"][data-row-id="a3"]').click();
    await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
    await expect(page.locator('[data-qa="halted-hatch"]')).toBeVisible();
    await expect(
      page.locator('[data-qa="ds-lab-row"][data-row-id="a3"] [data-qa="status-pill"]'),
    ).toHaveAttribute("data-tone", "halted");

    await revealDetailPanel(page);
    await expect(
      page
        .locator(
          '[data-panel-slot="detail"] [data-qa="status-pill"], [data-qa="panel-grid-drawer-detail"] [data-qa="status-pill"]',
        )
        .first(),
    ).toHaveAttribute("data-tone", "halted");
  });

  test("DS-LAYOUT · L1–L6 on D1/D2/D3 viewports", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "DS-LAYOUT" });
    // Projects already set D0–D3; when running under D1/D2/D3 assert against those sizes.
    // Also pin explicitly so a single-project local run still covers the required trio.
    const project = testInfo.project.name;
    if (project === "D1" || project === "D2" || project === "D3") {
      const vp = LAYOUT_VIEWPORTS[project];
      await page.setViewportSize(vp);
    } else if (project === "D0") {
      // Still assert layout on D0 for CI coverage continuity.
    } else {
      // D960 / D4-scale: exercise at D1 as the laptop baseline.
      await page.setViewportSize(LAYOUT_VIEWPORTS.D1);
    }

    await openRoute(page, DS_LAB, "full");
    await expect(page.locator('[data-qa="ds-lab"]')).toBeVisible();
    await expect(page.locator('[data-qa="panel-grid"]')).toBeVisible();

    const metrics = await assertLayoutL1toL6(page, DS_LAB, expect);
    expect(metrics.panelCount).toBeGreaterThanOrEqual(1);

    const mode = await page.locator('[data-qa="panel-grid"]').getAttribute("data-panel-mode");
    if (metrics.viewport.height > metrics.viewport.width) {
      expect(mode).toBe("portrait-stack");
      await expect(page.locator('[data-qa="panel-grid-splitter"]')).toBeVisible();
    } else if (metrics.viewport.width >= 1700) {
      // Content width is viewport − sidebar; wide shell still reaches three when ≥1700 container.
      // At 1920 with sidebar ~220, container is ~1700 — allow two or three.
      expect(["two", "three"]).toContain(mode);
    }
  });
});
