import { test, expect, type Page } from "@playwright/test";
import { assertLayoutL1toL6, formatLayoutFailure, measureLayout } from "../helpers/layout";
import { openRoute } from "../helpers/nav";
import type { LoungeExperience } from "../fixtures/types";

/**
 * KB-UI-1a — measurement-based layout gates for the Vault map PanelGrid.
 * Runs on every Playwright display project (D0–D3, D960, D4-scale).
 * No screenshot comparison (OPEN-6).
 *
 * L1–L6 are asserted only when leaf PanelGrid columns hold real content
 * (pages / dead-symbol rows / experience cards) — not stretched empty states.
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

async function closePanelDrawer(page: Page) {
  const closeBtn = page.locator('[data-qa="close-panel-drawer"]');
  if (await closeBtn.isVisible()) {
    await closeBtn.click();
    await expect(page.locator('[data-qa="panel-grid-drawer"]')).toHaveCount(0);
  }
}

async function revealExperiences(page: Page) {
  const inline = page.locator('[data-panel-slot="inspector"]');
  if (await inline.isVisible()) {
    return;
  }
  await closePanelDrawer(page);
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
  await closePanelDrawer(page);
  const openBtn = page.locator('[data-qa="open-detail-drawer"]');
  await expect(openBtn).toBeVisible();
  await openBtn.click();
  await expect(page.locator('[data-qa="panel-grid-drawer-detail"]')).toBeVisible();
}

/** Seed >25 active experiences for one project so the in-column pager appears (K4). */
async function seedExperiencesForPager(page: Page, projectId: string, count: number) {
  const rows: LoungeExperience[] = Array.from({ length: count }, (_, i) => ({
    id: `exp-pager-seed-${i + 1}`,
    type: "experience",
    agent: "lounge-kernel",
    project_id: projectId,
    adr_summary: `Pager seed ADR #${i + 1}`,
    outcome: "success",
    related_task_id: null,
    tags: ["pager"],
    created_at: new Date(Date.UTC(2026, 9, 1, 12, i % 60, 0)).toISOString(),
    status: "active",
    reviewed: true,
    use_count: 1,
    last_used_at: new Date(Date.UTC(2026, 9, 1, 12, i % 60, 0)).toISOString(),
    archived_at: null,
    archived_by: null,
    is_pinned: false,
    updated_at: new Date(Date.UTC(2026, 9, 1, 12, i % 60, 0)).toISOString(),
    original_content: null,
  }));
  await page.evaluate((seed) => {
    const fixture = (
      window as Window & { __QA_FIXTURE__?: { experiences: Array<Record<string, unknown>> } }
    ).__QA_FIXTURE__;
    if (!fixture?.experiences) {
      throw new Error("QA fixture missing experiences");
    }
    const existing = new Set(fixture.experiences.map((row) => String(row.id)));
    for (const row of seed) {
      if (!existing.has(row.id)) {
        fixture.experiences.push(row as unknown as Record<string, unknown>);
      }
    }
  }, rows);
}

test.describe("KB-UI-1a Vault map layout", () => {
  test("VL-01 · L1–L6 with real column content after drill-down", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VL-01" });
    await openRoute(page, "/vault", "full");
    await expect(page.locator('[data-qa="panel-grid"]')).toBeVisible();

    // Empty detail: natural-size copy centered by the pane — not a stretched filler.
    await revealDetail(page);
    const empty = page.locator('[data-qa="vault-dead-empty"] [data-qa="layout-empty"]');
    await expect(empty).toBeVisible();
    const emptyBox = await empty.boundingBox();
    const pane = page.locator('[data-qa="vault-dead-pane"]');
    const paneBox = await pane.boundingBox();
    expect(emptyBox).toBeTruthy();
    expect(paneBox).toBeTruthy();
    expect(emptyBox!.height).toBeLessThan((paneBox!.height || 1) * 0.45);

    await closePanelDrawer(page);
    await openVaultProject(page, "Agent-Lounge-OS");
    await revealDetail(page);
    await revealExperiences(page);
    // Real content: virtual page rows + dead-symbol rows + experience cards.
    await expect(page.locator('[data-qa="vault-page-row"]').first()).toBeVisible();
    await expect(
      page.locator('[data-qa="vault-dead-row"], [data-qa="experience-card"]').first(),
    ).toBeVisible();

    const metrics = await assertLayoutL1toL6(page, "/vault", expect);
    testInfo.annotations.push({
      type: "layout-pages",
      description: formatLayoutFailure(metrics),
    });
    expect(metrics.l2_sparseInterior).toBe(false);
    expect(metrics.l3_largestEmptyRatio).toBeLessThanOrEqual(0.15);
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
      await expect(page.locator('[data-qa="panel-grid-splitter"]')).toHaveCount(0);
    } else if (vp.width === 1920 && vp.height === 1080) {
      expect(mode).toBe("three");
      const slots = ["list", "detail", "inspector"] as const;
      for (const slot of slots) {
        await expect(page.locator(`[data-panel-slot="${slot}"]`)).toBeVisible();
      }
      // Fill columns before L3 — empty detail would leave honest sparse interior, not L3 area.
      await openVaultProject(page, "Agent-Lounge-OS");
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
      expect(widths.detail / widths.list).toBeGreaterThan(1.35);
      expect(widths.detail / widths.list).toBeLessThan(1.65);
      expect(widths.inspector / widths.list).toBeGreaterThan(0.85);
      expect(widths.inspector / widths.list).toBeLessThan(1.15);

      const metrics = await measureLayout(page, "/vault");
      expect(metrics.l3_pass, formatLayoutFailure(metrics)).toBe(true);
      expect(metrics.l3_largestEmptyRatio).toBeLessThanOrEqual(0.15);
      // No wrapping VaultPanel shell — only leaf chrome + PanelGrid slots.
      const wrapShell = await page
        .locator('section[data-qa="panel"]:not([data-panel-slot])')
        .count();
      expect(wrapShell).toBe(0);
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

  test("VL-03 · K4 pager in experiences column (>25 shown, ≤25 hidden)", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VL-03" });
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    await revealExperiences(page);

    // Default full fixture: Agent-Lounge-OS active experiences ≤25 → no pager.
    await expect(page.locator('[data-qa="vault-experience-header"]')).toBeVisible();
    await expect(page.locator('[data-qa="vault-experience-pager"]')).toHaveCount(0);
    await expect(
      page.locator('[data-panel-slot="vault-map-footer"] [data-qa="vault-experience-pager"]'),
    ).toHaveCount(0);

    // Seed enough rows for a second page (PAGE_SIZE=25) and refresh via archived toggle
    // (reload would re-clone the pristine fixture and drop the seed).
    await seedExperiencesForPager(page, "Agent-Lounge-OS", 30);
    const archiveToggle = page.getByRole("button", { name: /Show Archived|Hide Archived/i });
    await expect(archiveToggle).toBeVisible();
    await archiveToggle.click();
    await expect(page.locator('[data-qa="experience-card"]').first()).toBeVisible();
    const pager = page.locator('[data-qa="vault-experience-pager"]');
    await expect(pager).toBeVisible();
    await expect(
      page.locator('[data-panel-slot="vault-map-footer"] [data-qa="vault-experience-pager"]'),
    ).toHaveCount(0);
    // Pager must live under the experiences pane or its drawer — not the map chrome.
    const inExperiences = await page
      .locator(
        '[data-qa="vault-experiences-pane"] [data-qa="vault-experience-pager"], [data-qa="panel-grid-drawer-inspector"] [data-qa="vault-experience-pager"]',
      )
      .count();
    expect(inExperiences).toBeGreaterThan(0);
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
