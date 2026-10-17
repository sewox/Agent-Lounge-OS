import { test, expect } from "@playwright/test";
import { assertLayoutL1toL6, LAYOUT_VIEWPORTS } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

const ROUTES = [
  { path: "/dashboard" },
  { path: "/vault" },
  { path: "/health" },
  { path: "/settings" },
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

    await openRoute(page, "/settings", "full");
    expect(await page.evaluate(() => "__TAURI_INTERNALS__" in window)).toBeTruthy();

    // L1–L6 on a dense route at this viewport (before Halt chrome).
    await assertLayoutL1toL6(page, "/settings", expect);

    const haltBtn = page.locator('[data-qa="halt-toggle"]');
    await expect(haltBtn).toBeVisible();
    await expect(haltBtn).toHaveText(/Halt|Durdur/i);
    await haltBtn.click();

    const haltDialog = page.locator('[data-qa="halt-confirm-dialog"]');
    await expect(haltDialog).toBeVisible();
    await expect(
      haltDialog.getByText(/does not stop|Durmaz|external agent|harici ajan/i),
    ).toBeVisible();
    // Escape closes the dialog (a11y); focus trap lands on reason input or cancel.
    await page.locator('[data-qa="halt-dialog-cancel"]').focus();
    await page.keyboard.press("Escape");
    await expect(haltDialog).toHaveCount(0);

    await haltBtn.click();
    await expect(haltDialog).toBeVisible();
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

    // CT8: banner visible on every page AND L1–L6 while halted (correct route arg).
    for (const route of ROUTES) {
      await page.locator(`[data-qa="sidebar"] a[href="${route.path}"]`).click();
      await expect(page).toHaveURL(new RegExp(`${route.path.replace("/", "\\/")}`));
      await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
      await expect(page.locator('[data-qa="halt-toggle"]')).toBeVisible();
      await assertLayoutL1toL6(page, route.path, expect);
    }

    await page.locator('[data-qa="halt-toggle"]').click();
    const resumeDialog = page.locator('[data-qa="resume-confirm-dialog"]');
    await expect(resumeDialog).toBeVisible();
    await page.locator('[data-qa="halt-dialog-confirm"]').click();
    await expect(page.locator('[data-qa="halted-banner"]')).toHaveCount(0);
    await expect(page.locator('[data-qa="halt-toggle"]')).toHaveText(/Halt|Durdur/i);

    // L1–L6 again after Resume at the same viewport (D0–D3 projects).
    await page.locator('[data-qa="sidebar"] a[href="/settings"]').click();
    await expect(page).toHaveURL(/\/settings/);
    await assertLayoutL1toL6(page, "/settings", expect);
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

  test("CTL-03 · Halt stacks above SourceUnverified approval; Approve still works", async ({
    page,
  }) => {
    test.info().annotations.push({ type: "id", description: "CTL-03" });
    await openRoute(page, "/dashboard", "full");

    // Seed a pending SourceUnverified approval first (in-flight/pending not cancelled).
    await page.evaluate(() => {
      const f = window.__QA_FIXTURE__;
      if (!f) throw new Error("missing fixture");
      f.pendingApprovals = [
        {
          task_id: "ctl03-halted-approval",
          summary: "approve while halted",
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

    const approval = page.locator('[data-qa="approval-banner"]');
    await expect(approval).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('[data-qa="external-trigger-warning"]')).toBeVisible();

    // Engage Halt — approval chrome must remain visible (stacked, not exclusive).
    await page.locator('[data-qa="halt-toggle"]').click();
    await expect(page.locator('[data-qa="halt-confirm-dialog"]')).toBeVisible();
    await page.locator('[data-qa="halt-dialog-confirm"]').click();
    await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
    await expect(approval).toBeVisible();
    await expect(page.locator('[data-qa="external-trigger-warning"]')).toBeVisible();

    await approval.getByRole("button", { name: /Approve|Onayla/i }).click();
    await expect(page.locator('[data-qa="approval-banner"]')).toHaveCount(0, {
      timeout: 5_000,
    });
    // Halt banner remains after acting on the approval.
    await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible();
  });

  test("CTL-04 · get_halt_state reject → fail-closed UI + Resume reachable", async ({
    page,
  }) => {
    test.info().annotations.push({ type: "id", description: "CTL-04" });
    // Seed before mock init so the first get_halt_state rejects.
    await page.addInitScript(() => {
      sessionStorage.setItem("__QA_REJECT_HALT_STATE__", "halt_state JSON parse failed");
    });
    await openRoute(page, "/dashboard", "full");

    await expect(page.locator('[data-qa="halted-banner"]')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('[data-qa="halt-load-error"]')).toBeVisible();
    await expect(page.locator('[data-qa="halt-load-error"]')).toContainText(
      /Could not read Halt state|Durdurma durumu okunamadı|fail-closed/i,
    );
    await expect(page.locator('[data-qa="halt-load-error-strip"]')).toBeVisible();
    await expect(page.locator('[data-qa="halt-status-strip"] [data-qa="status-pill"]')).toHaveAttribute(
      "data-tone",
      "halted",
    );
    const haltBtn = page.locator('[data-qa="halt-toggle"]');
    await expect(haltBtn).toHaveText(/Resume|Devam/i);

    await haltBtn.click();
    const resumeDialog = page.locator('[data-qa="resume-confirm-dialog"]');
    await expect(resumeDialog).toBeVisible();
    await page.locator('[data-qa="halt-dialog-confirm"]').click();
    await expect(page.locator('[data-qa="halted-banner"]')).toHaveCount(0);
    await expect(haltBtn).toHaveText(/Halt|Durdur/i);
  });

  test("CTL-05 · Halt dialog Tab / Shift+Tab focus wrap", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "CTL-05" });
    await openRoute(page, "/dashboard", "full");
    await page.locator('[data-qa="halt-toggle"]').click();
    const dialog = page.locator('[data-qa="halt-confirm-dialog"]');
    await expect(dialog).toBeVisible();

    const reason = page.locator('[data-qa="halt-reason-input"]');
    const cancel = page.locator('[data-qa="halt-dialog-cancel"]');
    const confirm = page.locator('[data-qa="halt-dialog-confirm"]');

    await reason.focus();
    await expect(reason).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(cancel).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(confirm).toBeFocused();
    // Wrap forward: last → first
    await page.keyboard.press("Tab");
    await expect(reason).toBeFocused();
    // Wrap backward: first → last
    await page.keyboard.press("Shift+Tab");
    await expect(confirm).toBeFocused();
  });
});
