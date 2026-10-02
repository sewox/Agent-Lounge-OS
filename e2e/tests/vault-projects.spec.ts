import { test, expect, type Page } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import { measureLayout, formatLayoutFailure } from "../helpers/layout";

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

test.describe("VP — vault project grouping", () => {
  test("VP-01 · Semantic map lists projects not individual pages", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    const map = page.locator('[data-qa="vault-semantic-map"]');
    await expect(map).toHaveAttribute("data-view", "projects");
    const projects = page.locator('[data-qa="vault-project-row"]');
    await expect(projects).toHaveCount(3);
    // Must not dump hundreds of page rows at the top level.
    await expect(page.locator('[data-qa="vault-page-row"]')).toHaveCount(0);
    const text = await map.innerText();
    expect(text).toMatch(/Agent-Lounge-OS/i);
    expect(text).toMatch(/EchoMind/i);
  });

  test("VP-02 · Filters persist in session storage and reset", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await page.locator('[data-qa="vault-filter-name"]').fill("Echo");
    await page.locator('[data-qa="vault-filter-min-pages"]').fill("10");
    await page.waitForTimeout(200);
    await expect(page.locator('[data-qa="vault-project-row"]')).toHaveCount(1);
    await expect(page.locator('[data-qa="vault-project-row"]')).toContainText(/EchoMind/i);

    const stored = await page.evaluate(() =>
      window.localStorage.getItem("al-os-vault-project-filters"),
    );
    expect(stored).toMatch(/Echo/);

    await page.reload({ waitUntil: "domcontentloaded" });
    await openRoute(page, "/vault", "full");
    await expect(page.locator('[data-qa="vault-filter-name"]')).toHaveValue("Echo");
    await expect(page.locator('[data-qa="vault-project-row"]')).toHaveCount(1);

    await page.locator('[data-qa="vault-filter-reset"]').click();
    await expect(page.locator('[data-qa="vault-filter-name"]')).toHaveValue("");
    await expect(page.locator('[data-qa="vault-project-row"]')).toHaveCount(3);
  });

  test("VP-03 · Double-click drill-down opens page list", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    const row = page
      .locator('[data-qa="vault-project-row"]')
      .filter({ hasText: /Agent-Lounge-OS/i })
      .first();
    await row.dblclick();
    await expect(page.locator('[data-qa="vault-semantic-map"]')).toHaveAttribute(
      "data-view",
      "pages",
    );
    await expect(page.locator('[data-qa="vault-page-list"]')).toBeVisible();
    expect(await page.locator('[data-qa="vault-page-row"]').count()).toBeGreaterThan(10);
  });

  test("VP-04 · Search in project page list (debounced)", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    await page.locator('[data-qa="vault-page-search"]').fill("page_007");
    await page.waitForTimeout(350);
    const rows = page.locator('[data-qa="vault-page-row"]');
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText(/page_007/i);
  });

  test("VP-05 · Back navigation returns to project list", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "EchoMind");
    await page.locator('[data-qa="vault-back-projects"]').click();
    await expect(page.locator('[data-qa="vault-semantic-map"]')).toHaveAttribute(
      "data-view",
      "projects",
    );
    await expect(page.locator('[data-qa="vault-project-row"]')).toHaveCount(3);
  });

  test("VP-06 · Keyboard Enter / Aç button opens project", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    const row = page
      .locator('[data-qa="vault-project-row"]')
      .filter({ hasText: /codebase-memory-mcp/i })
      .first();
    await row.focus();
    await page.keyboard.press("Enter");
    await expect(page.locator('[data-qa="vault-semantic-map"]')).toHaveAttribute(
      "data-view",
      "pages",
    );
    await page.locator('[data-qa="vault-back-projects"]').click();
    await page
      .locator('[data-qa="vault-project-row"]')
      .filter({ hasText: /Agent-Lounge-OS/i })
      .locator('[data-qa="vault-project-open"]')
      .click();
    await expect(page.locator('[data-qa="vault-page-list"]')).toBeVisible();
  });

  test("VP-LAYOUT · Vault fills width/height on project + page views", async ({
    page,
  }, testInfo) => {
    await openRoute(page, "/vault", "full");
    const m1 = await measureLayout(page, "/vault");
    // Project list may be short (few repos) — require panel geometry, not dense interior.
    expect(m1.l1_pass && m1.l3_pass, formatLayoutFailure(m1)).toBe(true);
    await openVaultProject(page, "Agent-Lounge-OS");
    const m2 = await measureLayout(page, "/vault");
    testInfo.annotations.push({ type: "layout", description: formatLayoutFailure(m2) });
    expect(m2.l1_pass && m2.l3_pass && !m2.l2_sparseInterior, formatLayoutFailure(m2)).toBe(
      true,
    );
  });
});
