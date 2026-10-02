import { openRoute } from "../helpers/nav";
import { expect, test } from "@playwright/test";

test.describe("fresh install · optional LMR/NATS", () => {
  test("FI-01 · empty fixture shows optional banner, not SERVICE DEGRADED", async ({
    page,
  }) => {
    await openRoute(page, "/dashboard", "empty");
    await page.waitForTimeout(600);

    await expect(page.locator('[data-qa="service-degraded-banner"]')).toHaveCount(0);
    await expect(page.locator('[data-qa="sidebar-service-degraded"]')).toHaveCount(0);

    const optionalBanner = page.locator('[data-qa="service-optional-banner"]');
    await expect(optionalBanner).toBeVisible();
    const bannerText = await optionalBanner.innerText();
    expect(bannerText).toMatch(/Optional runtime|İsteğe bağlı runtime/i);
    expect(bannerText).not.toMatch(/Service Degraded|Servis bozuldu/i);
    expect(bannerText).toMatch(/LMR|NATS/);

    const sidebarOptional = page.locator('[data-qa="sidebar-service-optional"]');
    await expect(sidebarOptional).toBeVisible();
    const sideText = await sidebarOptional.innerText();
    expect(sideText).toMatch(/Not installed|Kurulu değil/i);
    expect(sideText).not.toMatch(/SERVICE DEGRADED|Servis bozuldu/i);

    const sidebar = await page.locator('[data-qa="sidebar"]').innerText();
    expect(sidebar).toMatch(/Not installed|Kurulu değil/i);
    // Restart is for crashes, not missing optional binaries.
    const restartOnLmr = page.locator('[data-qa="daemon-LMR"] button');
    await expect(restartOnLmr).toHaveCount(0);
  });

  test("FI-02 · Settings diagnostics documents kernel log path", async ({ page }) => {
    await openRoute(page, "/settings", "empty");
    const panel = page.locator('[data-qa="diagnostics-panel"]');
    await expect(panel).toBeVisible();
    await expect(page.locator('[data-qa="diagnostics-kernel-log"]')).toContainText(
      /logs\/kernel\.log/,
    );
  });
});

test.describe("palette i18n locale parity", () => {
  test("PL-01 · English UI shows English palette footer (no TR leftovers)", async ({
    page,
  }) => {
    await openRoute(page, "/dashboard", "full");
    await page
      .locator('[data-qa="locale-switch"]')
      .getByRole("radio", { name: /English|EN/i })
      .click();
    await page.waitForTimeout(200);
    await page.keyboard.press("Control+k");
    const footer = page.locator('[data-qa="palette-footer"]');
    await expect(footer).toBeVisible();
    const text = await footer.innerText();
    expect(text).toMatch(/select/i);
    expect(text).toMatch(/run/i);
    expect(text).not.toMatch(/seç|çalıştır/);
  });

  test("PL-02 · Turkish UI shows Turkish palette footer", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await page
      .locator('[data-qa="locale-switch"]')
      .getByRole("radio", { name: /TR|Türkçe|Turkish/i })
      .click();
    await page.waitForTimeout(200);
    await page.keyboard.press("Control+k");
    const footer = page.locator('[data-qa="palette-footer"]');
    await expect(footer).toBeVisible();
    const text = await footer.innerText();
    expect(text).toMatch(/seç/);
    expect(text).toMatch(/çalıştır/);
    expect(text).not.toMatch(/\bselect\b/i);
  });
});
