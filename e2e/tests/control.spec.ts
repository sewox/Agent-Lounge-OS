import { test, expect } from "@playwright/test";
import { assertLayoutL1toL6, LAYOUT_VIEWPORTS } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

const ROUTES = [
  { path: "/dashboard", label: /Dashboard|Gösterge/i },
  { path: "/vault", label: /Vault|Kasa/i },
  { path: "/health", label: /Health|Sağlık/i },
  { path: "/settings", label: /Settings|Ayarlar/i },
] as const;

test.describe("PR-Control-1 Halt + deep-link approval chrome", () => {
  test("CTL-01 · Halt/Resume dialogs + banner on D0–D3 with L1–L6", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "CTL-01" });
    const project = testInfo.project.name;
    if (project === "D1" || project === "D2" || project === "D3") {
      await page.setViewportSize(LAYOUT_VIEWPORTS[project]);
    }

    await openRoute(page, "/dashboard", "full");
    expect(await page.evaluate(() => "__TAURI_INTERNALS__" in window)).toBeTruthy();

    const haltBtn = page.locator('[data-qa="halt-toggle"]');
    await expect(haltBtn).toBeVisible();
    await expect(haltBtn).toHaveText(/Halt|Durdur/i);
    await haltBtn.click();

    const haltDialog = page.locator('[data-qa="halt-confirm-dialog"]');
    await expect(haltDialog).toBeVisible();
    await expect(
      haltDialog.getByText(/does not stop|Durmaz|external agent|harici ajan/i),
    ).toBeVisible();
    await page.locator('[data-qa="halt-reason-input"]').fill("e2e maintenance");
    await page.locator('[data-qa="halt-dialog-confirm"]').click();

    const banner = page.locator('[data-qa="halted-banner"]');
    await expect(banner).toBeVisible();
    await expect(page.locator('[data-qa="halted-hatch"]')).toBeVisible();
    await expect(page.locator('[data-qa="halted-banner-details"]')).toContainText(
      /e2e maintenance/,
    );
    await expect(
      banner.getByText(/does not stop|Durmaz|Cursor|Antigravity|Claude Desktop|Grok Bot/i),
    ).toBeVisible();
    await expect(
      page.locator('[data-qa="halt-status-strip"] [data-qa="status-pill"]'),
    ).toHaveAttribute("data-tone", "halted");
    await expect(haltBtn).toHaveText(/Resume|Devam/i);

    // Client-side nav keeps the same mock IPC (halt state persists).
    for (const route of ROUTES) {
      await page.locator(`[data-qa="sidebar"] a[href="${route.path}"]`).click();
      await expect(page).toHaveURL(new RegExp(`${route.path.replace("/", "\\/")}`));
      await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
      await expect(page.locator('[data-qa="halt-toggle"]')).toBeVisible();
      await assertLayoutL1toL6(page, expect);
    }

    await page.locator('[data-qa="halt-toggle"]').click();
    const resumeDialog = page.locator('[data-qa="resume-confirm-dialog"]');
    await expect(resumeDialog).toBeVisible();
    await page.locator('[data-qa="halt-dialog-confirm"]').click();
    await expect(page.locator('[data-qa="halted-banner"]')).toHaveCount(0);
    await expect(page.locator('[data-qa="halt-toggle"]')).toHaveText(/Halt|Durdur/i);
  });

  test("CTL-02 · External-trigger warning on source_unverified deep-link approval", async ({
    page,
  }) => {
    test.info().annotations.push({ type: "id", description: "CTL-02" });
    await openRoute(page, "/dashboard", "full");

    await page.evaluate(() => {
      const f = window.__QA_FIXTURE__;
      if (!f) throw new Error("missing fixture");
      f.pendingApprovals = [
        {
          task_id: "ctl02-deeplink-task",
          summary: "hello from deeplink",
          from_agent: "deep-link",
          to_agent: "cursor",
          kind: "source_unverified",
          reason:
            "Dış Kaynaklı Tetikleme · source_verified=false — deep-link dispatch · PENDING_APPROVAL",
          expires_at: new Date(Date.now() + 90_000).toISOString(),
          timeout_secs: 90,
        },
      ];
      document.dispatchEvent(new Event("visibilitychange"));
    });

    const banner = page.locator('[data-qa="approval-banner"]');
    await expect(banner).toBeVisible({ timeout: 5_000 });
    const warning = page.locator('[data-qa="external-trigger-warning"]');
    await expect(warning).toBeVisible();
    await expect(warning).toContainText(/Dış Kaynaklı Tetikleme|External trigger/i);
    await expect(banner.getByRole("button", { name: /Approve|Onayla/i })).toBeVisible();
  });
});
