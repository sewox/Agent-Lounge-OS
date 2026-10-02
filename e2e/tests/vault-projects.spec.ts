import { test, expect, type Page } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import { measureLayout, formatLayoutFailure } from "../helpers/layout";
import {
  attachHydrationConsoleCollector,
  isHydrationConsoleMessage,
  withHydrationProfile,
  openRouteHydration,
} from "../helpers/hydration";

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
    // 3 indexed + 1 experiences-only (solo-import)
    await expect(projects).toHaveCount(4);
    await expect(page.locator('[data-qa="vault-page-row"]')).toHaveCount(0);
    const text = await map.innerText();
    expect(text).toMatch(/Agent-Lounge-OS/i);
    expect(text).toMatch(/EchoMind/i);
    expect(text).toMatch(/solo-import/i);
  });

  test("VP-02 · Filters persist in localStorage and reset", async ({ page }) => {
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
    await expect(page.locator('[data-qa="vault-project-row"]')).toHaveCount(4);
  });

  test("VP-03 · Drill-down virtualizes and reaches last page past 500", async ({ page }) => {
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
    const totalText = await page.locator('[data-qa="vault-page-total"]').innerText();
    const total = Number((totalText.match(/(\d+)/) || [])[1] || 0);
    expect(total).toBeGreaterThanOrEqual(806);

    const domRows = page.locator('[data-qa="vault-page-row"]');
    const visibleCount = await domRows.count();
    expect(visibleCount).toBeGreaterThan(10);
    expect(visibleCount).toBeLessThan(80);

    // Scroll through windows past the 500 boundary to the last row.
    const list = page.locator('[data-qa="vault-page-list"]');
    for (let i = 0; i < 16; i++) {
      await list.evaluate((el) => {
        el.scrollTop = el.scrollHeight;
      });
      await page.waitForTimeout(250);
    }
    // Last generated page is page_805.rs (0..805); may coexist with semantic files.
    await expect(
      page.locator('[data-qa="vault-page-row"]').filter({ hasText: /page_805/i }),
    ).toBeVisible({ timeout: 15_000 });
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
    await expect(page.locator('[data-qa="vault-project-row"]')).toHaveCount(4);
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

  test("VP-07 · page_count matches drill-down total", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    const row = page
      .locator('[data-qa="vault-project-row"]')
      .filter({ hasText: /Agent-Lounge-OS/i })
      .first();
    const meta = await row.innerText();
    const match = meta.match(/(\d+)\s*pages/i) || meta.match(/(\d+)\s*sayfa/i);
    expect(match).toBeTruthy();
    const listed = Number(match![1]);
    await row.locator('[data-qa="vault-project-open"]').click();
    const totalText = await page.locator('[data-qa="vault-page-total"]').innerText();
    const totalMatch = totalText.match(/(\d+)/);
    expect(Number(totalMatch![1])).toBe(listed);
    expect(listed).toBeGreaterThanOrEqual(806);
  });

  test("VP-08 · experiences-only imported project appears", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    const row = page.locator('[data-qa="vault-project-row"]').filter({ hasText: /solo-import/i });
    await expect(row).toBeVisible();
    await expect(row).toContainText(/0\s*pages|0\s*sayfa/i);
  });

  test("VP-09 · projects error UI + retry", async ({ page }) => {
    await page.addInitScript(() => {
      (window as Window & { __QA_VAULT_FAIL__?: boolean }).__QA_VAULT_FAIL__ = true;
    });
    await openRoute(page, "/vault", "full");
    await expect(page.locator('[data-qa="vault-projects-error"]')).toBeVisible({ timeout: 10_000 });
    await page.evaluate(() => {
      (window as Window & { __QA_VAULT_FAIL__?: boolean }).__QA_VAULT_FAIL__ = false;
    });
    await page.locator('[data-qa="vault-projects-retry"]').click();
    await expect(page.locator('[data-qa="vault-project-row"]').first()).toBeVisible({
      timeout: 10_000,
    });
  });

  test("VP-10 · pages error UI on drill-down", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await page.evaluate(() => {
      (window as Window & { __QA_VAULT_FAIL__?: boolean }).__QA_VAULT_FAIL__ = true;
    });
    await openVaultProject(page, "EchoMind");
    await expect(page.locator('[data-qa="vault-pages-error"]')).toBeVisible({ timeout: 10_000 });
    await page.evaluate(() => {
      (window as Window & { __QA_VAULT_FAIL__?: boolean }).__QA_VAULT_FAIL__ = false;
    });
    await page.locator('[data-qa="vault-pages-retry"]').click();
    await expect(page.locator('[data-qa="vault-page-list"]')).toBeVisible({ timeout: 10_000 });
  });

  test("VP-11 · ResizeObserver rebinds after empty→loaded pages", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    // Delay first pages response so list mounts empty then fills.
    await page.evaluate(() => {
      const internals = window.__TAURI_INTERNALS__ as { invoke?: (c: string, a?: unknown) => Promise<unknown> };
      const orig = internals.invoke!.bind(internals);
      internals.invoke = async (cmd: string, args?: unknown) => {
        if (cmd === "list_project_pages") {
          await new Promise((r) => setTimeout(r, 400));
        }
        return orig(cmd, args);
      };
    });
    await openVaultProject(page, "Agent-Lounge-OS");
    await expect(page.locator('[data-qa="vault-page-list"]')).toBeVisible({ timeout: 10_000 });
    const height = await page.locator('[data-qa="vault-page-list"]').evaluate((el) => el.clientHeight);
    expect(height).toBeGreaterThan(320);
  });

  test("VP-LAYOUT · Vault fills width/height on project + page views", async ({
    page,
  }, testInfo) => {
    await openRoute(page, "/vault", "full");
    const m1 = await measureLayout(page, "/vault");
    testInfo.annotations.push({ type: "layout-projects", description: formatLayoutFailure(m1) });
    expect(
      m1.l1_pass && m1.l3_pass && !m1.l2_sparseInterior,
      formatLayoutFailure(m1),
    ).toBe(true);
    await openVaultProject(page, "Agent-Lounge-OS");
    const m2 = await measureLayout(page, "/vault");
    testInfo.annotations.push({ type: "layout-pages", description: formatLayoutFailure(m2) });
    expect(m2.l1_pass && m2.l3_pass && !m2.l2_sparseInterior, formatLayoutFailure(m2)).toBe(
      true,
    );
  });

  test("VP-12 · Page expand shows symbols + dead-only filter (TR/EN)", async ({
    browser,
  }) => {
    for (const [locale, deadLabel] of [
      ["tr-TR", /Sadece ölü semboller/i],
      ["en-US", /Dead symbols only/i],
    ] as const) {
      const context = await browser.newContext({ locale });
      const page = await context.newPage();
      await openRoute(page, "/vault", "full");
      await openVaultProject(page, "Agent-Lounge-OS");
      const dispatcher = page
        .locator('[data-qa="vault-page-row"]')
        .filter({ hasText: /dispatcher\.rs/i })
        .first();
      if (await dispatcher.count()) {
        await dispatcher.click();
      } else {
        await page.locator('[data-qa="vault-page-row"]').first().click();
      }
      await expect(page.locator('[data-qa="vault-page-symbols"]')).toBeVisible();
      await expect(page.locator('[data-qa="vault-page-symbols"]')).toContainText(deadLabel);
      const symbols = page.locator('[data-qa="vault-file-symbol"]');
      await expect(symbols.first()).toBeVisible();
      const before = await symbols.count();
      expect(before).toBeGreaterThan(0);
      await page.locator('[data-qa="vault-dead-only"]').check();
      const after = await symbols.count();
      expect(after).toBeLessThanOrEqual(before);
      await expect(
        page.locator('[data-qa="vault-file-symbol"][data-dead="true"]').first(),
      ).toBeVisible();
      await page.locator('[data-qa="vault-file-symbol"][data-dead="true"]').first().click();
      // Virtualization must still be intact under the symbols panel.
      expect(await page.locator('[data-qa="vault-page-row"]').count()).toBeLessThan(80);
      await context.close();
    }
  });

  test("VP-HYDRATION · TR locale /dashboard has no vault hydration noise", async ({
    browser,
  }) => {
    const profile = {
      id: "linux-tr-dashboard-vault",
      userAgent:
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
      locale: "tr-TR",
    };
    await withHydrationProfile(browser, profile, async (page) => {
      const entries = attachHydrationConsoleCollector(page);
      await openRouteHydration(page, "/dashboard", "full");
      // D0–D3 style viewports — sample a couple sizes for vault panel mount.
      for (const size of [
        { width: 1280, height: 720 },
        { width: 1512, height: 982 },
        { width: 1920, height: 1080 },
        { width: 1080, height: 1920 },
      ]) {
        await page.setViewportSize(size);
        await page.waitForTimeout(300);
      }
      const noise = entries.filter((e) => isHydrationConsoleMessage(e.text));
      expect(noise, noise.map((e) => e.text).join("\n")).toEqual([]);
    });
  });
});
