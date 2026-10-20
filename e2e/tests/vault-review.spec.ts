import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { VAULT_REVIEW_SEED_EXPERIENCES } from "../fixtures/full";
import { assertLayoutL1toL6 } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

/**
 * PR-Vault-Review-UI — runs on D0–D3 (no testIgnore).
 * Every state exercises assertLayoutL1toL6; D2 asserts three inline panels.
 */

const REVIEW = "/vault?view=review";

/** Inject review-queue seeds without changing FULL_FIXTURE (preserves EX-05). */
async function seedReviewQueue(page: Page) {
  await page.evaluate((rows) => {
    const fixture = (
      window as Window & { __QA_FIXTURE__?: { experiences: Array<Record<string, unknown>> } }
    ).__QA_FIXTURE__;
    if (!fixture) {
      throw new Error("__QA_FIXTURE__ missing");
    }
    const existing = new Set(fixture.experiences.map((row) => String(row.id)));
    for (const row of rows) {
      if (!existing.has(row.id)) {
        fixture.experiences.push(row as unknown as Record<string, unknown>);
      }
    }
  }, VAULT_REVIEW_SEED_EXPERIENCES);
  // Reload queue + refresh shell badge from mutated fixture.
  await page.locator('[data-qa="stats-strip-item"][data-stat-id="hygiene"]').click();
  await page.locator('[data-qa="stats-strip-item"][data-stat-id="pending"]').click();
  await expect(page.locator('[data-qa-vault-review="ready"]')).toBeVisible();
}

async function openReviewSeeded(page: Page) {
  await openRoute(page, REVIEW, "full");
  await expect(page.locator('[data-qa-vault-review="ready"]')).toBeVisible();
  await seedReviewQueue(page);
}

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
    await openReviewSeeded(page);
    await expect(page.locator('[data-qa="vault-review-list"]')).toBeVisible();
    // Select a dense fill row so detail/inspector panes have real interior (L2).
    const fillRow = page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-fill-1"]');
    await fillRow.locator("button").click();
    await revealDetail(page);
    await revealInspector(page);
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

  test("VR-LAYOUT · L1–L6 loading state", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-LAYOUT-loading" });
    await page.addInitScript(() => {
      (window as Window & { __QA_FORCE_REVIEW_LOADING__?: boolean }).__QA_FORCE_REVIEW_LOADING__ =
        true;
    });
    await openRoute(page, REVIEW, "full");
    await expect(page.locator('[data-qa-vault-review="loading"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, REVIEW, expect);
    expect(metrics.l1_pass).toBe(true);
    expect(metrics.l5_pass).toBe(true);
  });

  test("VR-LAYOUT · L1–L6 non-Tauri browser state", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-LAYOUT-browser" });
    await openRoute(page, REVIEW, "browser");
    await expect(page.locator('[data-qa-vault-review="browser"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, REVIEW, expect);
    expect(metrics.l1_pass).toBe(true);
    expect(metrics.l5_pass).toBe(true);
  });

  test("VR-01 · badge navigates to review + counter matches list", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-01" });
    // Non-interactive badge lives inside the vault nav Link → review view.
    await openRoute(page, "/dashboard", "full");
    const badge = page.locator('[data-qa="unreviewed-count"]');
    await expect(badge).toBeVisible();
    await expect(badge).toHaveAttribute("aria-hidden", "true");
    await badge.click();
    await expect(page).toHaveURL(/\/vault\?view=review/);
    await expect(page.locator('[data-qa-vault-review="ready"]')).toBeVisible();

    // Seed queue; StatsStrip pending, list rows, and pendingCount share one source.
    await seedReviewQueue(page);
    await expect(
      page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-fill-1"]'),
    ).toBeVisible();
    const pendingText = await page.locator('[data-qa="review-pending-count"]').innerText();
    const pendingCount = Number(/(\d+)/.exec(pendingText)?.[1] ?? "0");
    expect(pendingCount).toBeGreaterThan(1);
    const stripPending = page.locator(
      '[data-qa="stats-strip-item"][data-stat-id="pending"]',
    );
    await expect(stripPending).toContainText(String(pendingCount));
    const rows = page.locator('[data-qa="vault-review-row"]');
    await expect(rows).toHaveCount(pendingCount);
  });

  test("VR-01b · count IPC failure shows unknown badge (not silent 0)", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-01b" });
    await page.addInitScript(() => {
      (
        window as Window & { __QA_FORCE_UNREVIEWED_COUNT_ERROR__?: boolean }
      ).__QA_FORCE_UNREVIEWED_COUNT_ERROR__ = true;
    });
    await openRoute(page, "/dashboard", "full");
    await expect(page.locator('[data-qa="unreviewed-count-unknown"]')).toBeVisible();
    await expect(page.locator('[data-qa="unreviewed-count"]')).toHaveCount(0);
  });

  test("VR-02 · hygiene sorts first + approve/reject/undo", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-02" });
    await openReviewSeeded(page);
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
    await revealDetail(page);
    await page.locator('[data-qa="review-reject"]').click();
    await expect(plain).toHaveCount(0);

    await page.locator('[data-qa="stats-strip-item"][data-stat-id="rejected"]').click();
    await expect(
      page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-plain"]'),
    ).toBeVisible();
    await page
      .locator('[data-qa="vault-review-row"][data-experience-id="exp-review-plain"] button')
      .click();
    await revealDetail(page);
    await page.locator('[data-qa="review-undo"]').click();
    await page.locator('[data-qa="stats-strip-item"][data-stat-id="pending"]').click();
    await expect(
      page.locator('[data-qa="vault-review-row"][data-experience-id="exp-review-plain"]'),
    ).toBeVisible();
  });

  test("VR-03 · bulk confirm + keyboard Space/Enter/arrows", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-03" });
    await openReviewSeeded(page);
    await expect(page.locator('[data-qa="vault-review-list"]')).toBeVisible();
    const list = page.locator('[data-qa="vault-review-list"]');
    await list.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press(" ");
    await expect(page.locator('[data-qa="sticky-action-bar"]')).toBeVisible();
    await page.locator('[data-qa="review-bulk-reject"]').click();
    await expect(page.locator('[data-qa="vault-review-confirm"]')).toBeVisible();
    await page.locator('[data-qa="review-confirm-cancel"]').click();
    await expect(page.locator('[data-qa="vault-review-confirm"]')).toHaveCount(0);
    await list.focus();
    await page.keyboard.press("Enter");
    await revealDetail(page);
    await expect(
      page.locator(
        '[data-panel-slot="detail"] [data-qa="vault-review-detail"], [data-qa="panel-grid-drawer-detail"] [data-qa="vault-review-detail"]',
      ),
    ).toBeVisible();
  });

  test("VR-04 · axe main + nav + dotted unreviewed + no home-path leak", async ({
    page,
  }) => {
    test.info().annotations.push({ type: "id", description: "VR-04" });
    await openReviewSeeded(page);
    await expect(page.locator('[data-qa="unreviewed-badge"]').first()).toBeVisible();
    const badgeClass = await page
      .locator('[data-qa="unreviewed-badge"]')
      .first()
      .getAttribute("class");
    expect(badgeClass).toMatch(/border-dashed/);
    await revealDetail(page);
    await revealInspector(page);
    await expect(page.locator('[data-qa="review-inspector-masked-content"]')).toBeVisible();
    const bodyText = await page.locator("body").innerText();
    expect(bodyText).not.toMatch(/\/Users\//);
    expect(bodyText).not.toMatch(/\/home\//);
    expect(bodyText).not.toMatch(/\/var\/home\//);
    expect(bodyText).not.toMatch(/(^|[^a-z])\/root(\/|\s|$)/i);
    expect(bodyText).not.toMatch(/C:\\Users/i);
    expect(bodyText).not.toMatch(/C:\/Users/i);
    const mainResults = await new AxeBuilder({ page }).include("main").analyze();
    expect(mainResults.violations, JSON.stringify(mainResults.violations, null, 2)).toEqual(
      [],
    );
    await expect(page.locator('[data-qa="app-shell-nav"]')).toBeVisible();
    const navResults = await new AxeBuilder({ page })
      .include('[data-qa="app-shell-nav"]')
      .analyze();
    expect(navResults.violations, JSON.stringify(navResults.violations, null, 2)).toEqual([]);
  });

  test("VR-05 · StatsStrip filters + Halted still allows approve", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "VR-05" });
    await openReviewSeeded(page);
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
