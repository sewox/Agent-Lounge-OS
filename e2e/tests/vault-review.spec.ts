import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { assertLayoutL1toL6 } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

/**
 * PR-Vault-Review-UI — runs on D0–D3 (no testIgnore).
 * Every state exercises assertLayoutL1toL6; D2 asserts three inline panels.
 */

const REVIEW = "/vault?view=review";

async function revealDetail(page: Page) {
  const inline = page.locator('[data-panel-slot="detail"]');
  const openBtn = page.locator('[data-qa="open-detail-drawer"]');
  if (!(await inline.isVisible()) && (await openBtn.count()) > 0) {
    await openBtn.click();
  }
  await expect(
    page.locator('[data-panel-slot="detail"], [data-qa="panel-grid-drawer-detail"]'),
  ).toBeVisible();
}

async function revealInspector(page: Page) {
  const closeBtn = page.locator('[data-qa="close-panel-drawer"]');
  if (await closeBtn.isVisible()) {
    await closeBtn.click();
  }
  const inline = page.locator('[data-panel-slot="inspector"]');
  const openBtn = page.locator('[data-qa="open-inspector-drawer"]');
  if (!(await inline.isVisible()) && (await openBtn.count()) > 0) {
    await openBtn.click();
  }
  await expect(
    page.locator(
      '[data-panel-slot="inspector"], [data-qa="panel-grid-drawer-inspector"]',
    ),
  ).toBeVisible();
}

test.describe("Vault Review UI", () => {
  test("VR-LAYOUT · L1–L6 ready state", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "VR-LAYOUT-ready" });
    await openRoute(page, REVIEW, "full");
    await expect(page.locator('[data-qa-vault-review="ready"]')).toBeVisible();
    await expect(page.locator('[data-qa="vault-review-list"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, REVIEW, expect);
    expect(metrics.l1_pass).toBe(true);
    expect(metrics.l5_pass).toBe(true);

    if (testInfo.project.name === "D2") {
      const grid = page.locator('[data-qa="panel-grid"]');
      await expect(grid).toBeVisible();
      await expect(grid).toHaveAttribute("data-panel-mode", "three");
      await expect(page.locator('[data-panel-slot="list"]')).toBeVisible();
      await expect(page.locator('[data-panel-slot="detail"]')).toBeVisible();
      await expect(page.locator('[data-panel-slot="inspector"]')).toBeVisible();
    }
  });

  test("VR-LAYOUT · L1–L6 empty state", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-LAYOUT-empty" });
    await openRoute(page, REVIEW, "empty");
    await expect(page.locator('[data-qa-vault-review="empty"]')).toBeVisible();
    await expect(page.locator('[data-qa="vault-review-empty"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, REVIEW, expect);
    expect(metrics.l1_pass).toBe(true);
    expect(metrics.l5_pass).toBe(true);
  });

  test("VR-LAYOUT · L1–L6 error state", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-LAYOUT-error" });
    await page.addInitScript(() => {
      (window as Window & { __QA_FORCE_REVIEW_ERROR__?: boolean }).__QA_FORCE_REVIEW_ERROR__ =
        true;
    });
    await openRoute(page, REVIEW, "full");
    await expect(page.locator('[data-qa-vault-review="error"]')).toBeVisible();
    await expect(page.locator('[data-qa="vault-review-retry"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, REVIEW, expect);
    expect(metrics.l1_pass).toBe(true);
    expect(metrics.l5_pass).toBe(true);
  });

  test("VR-01 · badge navigates to review + counter matches list", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-01" });
    await openRoute(page, "/dashboard", "full");
    const badge = page.locator('[data-qa="unreviewed-count"]');
    await expect(badge).toBeVisible();
    const badgeCount = Number((await badge.innerText()).trim());
    expect(badgeCount).toBeGreaterThan(0);
    await page.locator('[data-qa="unreviewed-count"]').click();
    await expect(page).toHaveURL(/\/vault\?view=review/);
    await expect(page.locator('[data-qa-vault-review="ready"]')).toBeVisible();
    const pending = page.locator('[data-qa="review-pending-count"]');
    await expect(pending).toContainText(String(badgeCount));
    const stripPending = page.locator(
      '[data-qa="stats-strip-item"][data-stat-id="pending"]',
    );
    await expect(stripPending).toContainText(String(badgeCount));
    const rows = page.locator('[data-qa="vault-review-row"]');
    await expect(rows.first()).toBeVisible();
    expect(await rows.count()).toBe(badgeCount);
  });

  test("VR-02 · hygiene sorts first + approve/reject/undo", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-02" });
    await openRoute(page, REVIEW, "full");
    await expect(page.locator('[data-qa-vault-review="ready"]')).toBeVisible();
    const first = page.locator('[data-qa="vault-review-row"]').first();
    await expect(first).toHaveAttribute("data-hygiene", "true");
    await expect(first).toHaveAttribute("data-experience-id", "exp-review-hygiene");

    await first.locator("button").click();
    await revealDetail(page);
    await page.locator('[data-qa="review-approve"]').click();
    await expect(
      page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-hygiene"]'),
    ).toHaveCount(0);

    const plain = page.locator(
      '[data-qa="vault-review-row"][data-experience-id="exp-review-plain"]',
    );
    await plain.locator("button").click();
    await page.locator('[data-qa="review-reject"]').click();
    await expect(page.locator('[data-qa="vault-review-confirm"]')).toBeVisible();
    await page.locator('[data-qa="review-confirm-ok"]').click();
    await expect(plain).toHaveCount(0);

    await page.locator('[data-qa="stats-strip-item"][data-stat-id="rejected"]').click();
    await expect(
      page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-plain"]'),
    ).toBeVisible();
    await page
      .locator('[data-qa="vault-review-row"][data-experience-id="exp-review-plain"] button')
      .click();
    await page.locator('[data-qa="review-undo"]').click();
    await page.locator('[data-qa="stats-strip-item"][data-stat-id="pending"]').click();
    await expect(
      page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-plain"]'),
    ).toBeVisible();
  });

  test("VR-03 · bulk confirm + keyboard Space/Enter/arrows", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-03" });
    await openRoute(page, REVIEW, "full");
    await expect(page.locator('[data-qa="vault-review-list"]')).toBeVisible();
    const list = page.locator('[data-qa="vault-review-list"]');
    await list.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press(" ");
    await expect(page.locator('[data-qa="sticky-action-bar"]')).toBeVisible();
    await page.locator('[data-qa="review-bulk-approve"]').click();
    await expect(page.locator('[data-qa="vault-review-confirm"]')).toBeVisible();
    await page.locator('[data-qa="review-confirm-cancel"]').click();
    await expect(page.locator('[data-qa="vault-review-confirm"]')).toHaveCount(0);
    await page.keyboard.press("Enter");
    await revealDetail(page);
    await expect(page.locator('[data-qa="vault-review-detail"]')).toBeVisible();
  });

  test("VR-04 · axe main scope + dotted unreviewed badge", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-04" });
    await openRoute(page, REVIEW, "full");
    await expect(page.locator('[data-qa="unreviewed-badge"]').first()).toBeVisible();
    const badgeClass = await page
      .locator('[data-qa="unreviewed-badge"]')
      .first()
      .getAttribute("class");
    expect(badgeClass).toMatch(/border-dashed/);
    await revealDetail(page);
    await revealInspector(page);
    const results = await new AxeBuilder({ page }).include("main").analyze();
    expect(results.violations, JSON.stringify(results.violations, null, 2)).toEqual([]);
  });

  test("VR-05 · StatsStrip filters + Halted still allows approve", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-05" });
    await openRoute(page, REVIEW, "full");
    await page.locator('[data-qa="stats-strip-item"][data-stat-id="hygiene"]').click();
    await expect(
      page.locator('[data-qa="stats-strip-item"][data-stat-id="hygiene"]'),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      page.locator('[data-qa="vault-review-row"][data-hygiene="true"]').first(),
    ).toBeVisible();

    await page.evaluate(async () => {
      const w = window as Window & {
        __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
      };
      await w.__TAURI_INTERNALS__?.invoke("halt", { reason: "vault-review-test" });
    });
    await page.locator('[data-qa="stats-strip-item"][data-stat-id="pending"]').click();
    const row = page.locator('[data-qa="vault-review-row"]').first();
    await row.locator("button").click();
    await revealDetail(page);
    await page.locator('[data-qa="review-approve"]').click();
    // Approve must succeed while Halted (local user action).
    await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
  });
});
