import { expect, test } from "@playwright/test";
import { getIpcLog } from "../harness/tauri-mock";
import { openRoute } from "../helpers/nav";

test.describe("WS · Workspace scan → import → background index", () => {
  test("WS-01 · Scan discovers projects, imports, indexes without page reload", async ({
    page,
  }) => {
    await openRoute(page, "/dashboard", "empty");

    const scanBtn = page.getByRole("button", {
      name: /Index Workspace|Çalışma Alanını Tara/i,
    });
    await expect(scanBtn.first()).toBeVisible();

    await scanBtn.first().click();

    // Scan IPC must fire (dialog bypassed in mock via scan_workspace).
    await expect
      .poll(async () => {
        const log = await getIpcLog(page);
        return log.some((row) => row.cmd === "scan_workspace");
      }, { timeout: 8_000 })
      .toBeTruthy();

    // Notice / progress appears without reload.
    const banner = page.locator(
      '[data-qa="index-progress-banner"], [data-qa="index-notice-banner"]',
    );
    await expect(banner.first()).toBeVisible({ timeout: 8_000 });

    // Per-project job rows (queued → done via mock background events).
    await expect
      .poll(async () => page.locator('[data-qa="index-job-row"]').count(), {
        timeout: 8_000,
      })
      .toBeGreaterThan(0);

    // Background completion updates UI (event-driven).
    await expect
      .poll(
        async () => {
          const text = await page.locator("body").innerText();
          return /done|tamam|Imported|içe aktar/i.test(text);
        },
        { timeout: 10_000 },
      )
      .toBeTruthy();

    // Navigate via in-app links (preserve mock + rehydrate list_index_jobs on focus).
    await page.getByRole("link", { name: /Project Health|Proje Sağlığı|Health/i }).first().click();
    await page.waitForTimeout(400);
    await page.getByRole("link", { name: /Dashboard|Gösterge/i }).first().click();
    await page.waitForTimeout(400);

    const log = await getIpcLog(page);
    expect(
      log.some((row) => row.cmd === "list_index_jobs" || row.cmd === "scan_workspace"),
      "expected scan or job rehydrate IPC",
    ).toBeTruthy();
  });

  test("WS-02 · Empty workspace error is visible (no silent no-op)", async ({ page }) => {
    await openRoute(page, "/dashboard", "empty");

    // Override scan_workspace to fail with empty_workspace.
    await page.evaluate(() => {
      const internals = window.__TAURI_INTERNALS__ as {
        invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
      const original = internals.invoke.bind(internals);
      internals.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "scan_workspace") {
          throw new Error("empty_workspace: no projects found in workspace");
        }
        return original(cmd, args);
      };
    });

    const scanBtn = page.getByRole("button", {
      name: /Index Workspace|Çalışma Alanını Tara/i,
    });
    await scanBtn.first().click();

    const notice = page.locator('[data-qa="index-notice-banner"]');
    await expect(notice).toBeVisible({ timeout: 8_000 });
    await expect(notice).toContainText(
      /No projects found|proje bulunamadı|empty workspace/i,
    );
  });
});
