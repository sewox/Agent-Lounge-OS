import { test, expect, type Page } from "@playwright/test";
import { assertLayoutL1toL6, formatLayoutFailure, measureLayout } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

/**
 * KB-UI-1a — measurement-based layout gates for the Vault map PanelGrid.
 * Runs on every Playwright display project (D0–D3, D960, D4-scale).
 * No screenshot comparison (OPEN-6).
 */

async function openVaultProject(page: Page, name: string) {
  const row = page
    .locator('[data-qa="vault-project-row"]')
    .filter({ hasText: new RegExp(name, "i") })
    .first();
  await expect(row).toBeVisible();
  await row.locator('[data-qa="vault-project-open"]').click();
  await expect(page.locator('[data-qa="vault-semantic-map"]')).toHaveAttribute(
    "data-view",
    "pages",
  );
}

async function revealExperiences(page: Page) {
  const inline = page.locator('[data-panel-slot="inspector"]');
  if (await inline.isVisible()) {
    return;
  }
  const openBtn = page.locator('[data-qa="open-inspector-drawer"]');
  await expect(openBtn).toBeVisible();
  await openBtn.click();
  await expect(page.locator('[data-qa="panel-grid-drawer-inspector"]')).toBeVisible();
}

async function revealDetail(page: Page) {
  const inline = page.locator('[data-panel-slot="detail"]');
  if (await inline.isVisible()) {
    return;
  }
  const openBtn = page.locator('[data-qa="open-detail-drawer"]');
  await expect(openBtn).toBeVisible();
  await openBtn.click();
  await expect(page.locator('[data-qa="panel-grid-drawer-detail"]')).toBeVisible();
}

test.describe("KB-UI-1a Vault map layout", () => {
  test("VL-01 · L1–L6 on project list and page drill-down", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VL-01" });
    await openRoute(page, "/vault", "full");
    await expect(page.locator('[data-qa="panel-grid"]')).toBeVisible();

    const m1 = await assertLayoutL1toL6(page, "/vault", expect);
    testInfo.annotations.push({
      type: "layout-projects",
      description: formatLayoutFailure(m1),
    });

    await openVaultProject(page, "Agent-Lounge-OS");
    await revealExperiences(page);
    const m2 = await assertLayoutL1toL6(page, "/vault", expect);
    testInfo.annotations.push({
      type: "layout-pages",
      description: formatLayoutFailure(m2),
    });
    expect(m2.l2_sparseInterior).toBe(false);
  });

  test("VL-02 · Panel mode + column ratios match viewport", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VL-02" });
    await openRoute(page, "/vault", "full");
    const grid = page.locator('[data-qa="panel-grid"]');
    await expect(grid).toBeVisible();
    const mode = await grid.getAttribute("data-panel-mode");
    const vp = page.viewportSize()!;
    const isPortrait = vp.height > vp.width;

    if (isPortrait) {
      expect(mode).toBe("portrait-stack");
      await expect(page.locator('[data-panel-slot="list"]')).toBeVisible();
      await expect(page.locator('[data-panel-slot="detail"]')).toBeVisible();
      await expect(page.locator('[data-panel-slot="inspector"]')).toBeVisible();
      // Triple portrait stack — no list/detail splitter.
      await expect(page.locator('[data-qa="panel-grid-splitter"]')).toHaveCount(0);
    } else if (vp.width === 1920 && vp.height === 1080) {
      // D2: container ≥1600 after sidebar → three columns at 1:1.5:1.
      expect(mode).toBe("three");
      const slots = ["list", "detail", "inspector"] as const;
      for (const slot of slots) {
        await expect(page.locator(`[data-panel-slot="${slot}"]`)).toBeVisible();
      }
      const widths = await page.evaluate(() => {
        const read = (slot: string) => {
          const el = document.querySelector(`[data-panel-slot="${slot}"]`) as HTMLElement | null;
          return el?.getBoundingClientRect().width ?? 0;
        };
        return { list: read("list"), detail: read("detail"), inspector: read("inspector") };
      });
      expect(widths.list).toBeGreaterThan(0);
      expect(widths.detail).toBeGreaterThan(0);
      expect(widths.inspector).toBeGreaterThan(0);
      // detail ≈ 1.5 × list; inspector ≈ list (tolerance for gaps).
      expect(widths.detail / widths.list).toBeGreaterThan(1.35);
      expect(widths.detail / widths.list).toBeLessThan(1.65);
      expect(widths.inspector / widths.list).toBeGreaterThan(0.85);
      expect(widths.inspector / widths.list).toBeLessThan(1.15);

      const metrics = await measureLayout(page, "/vault");
      expect(metrics.l3_pass, formatLayoutFailure(metrics)).toBe(true);
      expect(metrics.l3_largestEmptyRatio).toBeLessThanOrEqual(0.15);
    } else if (mode === "two") {
      await expect(page.locator('[data-panel-slot="list"]')).toBeVisible();
      await expect(page.locator('[data-panel-slot="detail"]')).toBeVisible();
      await expect(page.locator('[data-panel-slot="inspector"]')).toHaveCount(0);
      await expect(page.locator('[data-qa="open-inspector-drawer"]')).toBeVisible();
      const widths = await page.evaluate(() => {
        const read = (slot: string) => {
          const el = document.querySelector(`[data-panel-slot="${slot}"]`) as HTMLElement | null;
          return el?.getBoundingClientRect().width ?? 0;
        };
        return { list: read("list"), detail: read("detail") };
      });
      expect(widths.detail / widths.list).toBeGreaterThan(1.35);
      expect(widths.detail / widths.list).toBeLessThan(1.65);
    } else {
      expect(mode).toBe("one");
      await expect(page.locator('[data-panel-slot="list"]')).toBeVisible();
      await expect(page.locator('[data-qa="open-detail-drawer"]')).toBeVisible();
      await expect(page.locator('[data-qa="open-inspector-drawer"]')).toBeVisible();
    }
  });

  test("VL-03 · Dead-symbol virtual list + pager location", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VL-03" });
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    await revealDetail(page);

    const deadList = page.locator('[data-qa="vault-dead-list"], [data-qa="vault-dead-empty"]');
    await expect(deadList.first()).toBeVisible();

    // No shared-footer pager — pager lives in the experiences column when pages > 1.
    const footerPager = page.locator('[data-qa="vault-map-footer"] [data-qa="vault-experience-pager"]');
    await expect(footerPager).toHaveCount(0);

    await revealExperiences(page);
    const header = page.locator('[data-qa="vault-experience-header"]');
    await expect(header).toBeVisible();

    // Pager is hidden when ≤25 experiences (single page).
    const experiencePager = page.locator('[data-qa="vault-experience-pager"]');
    const pagerCount = await experiencePager.count();
    if (pagerCount > 0) {
      await expect(experiencePager).toBeVisible();
      // Must sit inside the experiences pane / drawer, not the map footer.
      const inFooter = await page
        .locator('[data-qa="vault-map-footer"] [data-qa="vault-experience-pager"]')
        .count();
      expect(inFooter).toBe(0);
    }
  });

  test("VL-04 · D4-scale rem + page row height track UI scale", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VL-04" });
    await openRoute(page, "/vault", "full");
    if (testInfo.project.name === "D4-scale") {
      const rem = await page.evaluate(() =>
        parseFloat(getComputedStyle(document.documentElement).fontSize),
      );
      expect(rem, "D4-scale must apply 130% UI rem").toBeGreaterThan(16);
      const scaleAttr = await page.evaluate(
        () => document.documentElement.dataset.uiScale || "",
      );
      expect(scaleAttr).toBe("1.3");
    }

    await openVaultProject(page, "Agent-Lounge-OS");
    await expect(page.locator('[data-qa="vault-page-list"]')).toBeVisible({ timeout: 10_000 });
    const rowH = await page.locator('[data-qa="vault-page-row"]').first().evaluate((el) => {
      return (el as HTMLElement).getBoundingClientRect().height;
    });
    const expected = await page.evaluate(() => {
      const fs = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      return fs * 2.25;
    });
    expect(Math.abs(rowH - expected)).toBeLessThan(2);
  });
});
