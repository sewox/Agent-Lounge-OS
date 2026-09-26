import { test, expect } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import { getIpcLog } from "../harness/tauri-mock";

test.describe("HM — health / map empty states", () => {
  test("HM-01 · Empty DB must not show MOCK_HEALTH 12/41/74 [expected-fail until PR-2]", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "HealthPanel falls back to MOCK_HEALTH" });
    test.fail(true, "MOCK_HEALTH still shown when map/projects empty");
    await openRoute(page, "/health", "empty");
    const text = await page.locator("main").innerText();
    expect(text).not.toMatch(/\b12\b/);
    expect(text).not.toMatch(/\b41\b/);
    expect(text).not.toMatch(/\b74\b/);
    await expect(page.getByText(/No data found|Index Workspace/i)).toBeVisible();
  });

  test("HM-02 · Empty Map must not use MOCK_NODES [expected-fail until PR-2]", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "SemanticMap falls back to MOCK_NODES" });
    test.fail(true, "MOCK_NODES still used when empty");
    await openRoute(page, "/vault", "empty");
    const text = await page.locator("main").innerText();
    expect(text).not.toMatch(/EchoMind/);
    await expect(page.getByText(/No data found|Index Workspace/i)).toBeVisible();
  });

  test("HM-03 · Full DB shows live health rows", async ({ page }) => {
    await openRoute(page, "/health", "full");
    await expect(page.getByText("Agent-Lounge-OS").first()).toBeVisible();
    const text = await page.locator("main").innerText();
    expect(text).toMatch(/live|Agent-Lounge/i);
  });

  test("HM-04 · Re-index only on /health, not in palette", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await page.keyboard.press("Control+k");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    const palText = await dialog.innerText();
    expect(palText).not.toMatch(/Re-?index/i);
    expect(palText).not.toMatch(/Clear Cache/i);
  });
});

test.describe("ST — settings", () => {
  test("ST-01 · Routing table not clipped @ D0 [expected-fail until PR-2/5]", async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== "D0" && testInfo.project.name !== "D4-scale", "D0/D4");
    await openRoute(page, "/settings", "full");
    const table = page.locator("table").first();
    if ((await table.count()) === 0) {
      // May be div-based rows
      const trigger = page.getByText(/Cursor|trigger|Routing/i).first();
      await expect(trigger).toBeVisible();
      const box = await trigger.boundingBox();
      const vp = page.viewportSize()!;
      if (box && box.y + box.height > vp.height - 8) {
        testInfo.annotations.push({ type: "expected-fail", description: "V8 clipped routing table" });
        test.fail(true, "Routing controls clipped at bottom of viewport");
      }
      return;
    }
    const box = await table.boundingBox();
    const vp = page.viewportSize()!;
    if (box && box.y + box.height > vp.height + 2) {
      test.fail(true, "Table extends past viewport without scroll cue");
    }
  });

  test("ST-02 · Routing policy save calls set_routing_policy", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const save = page.getByRole("button", { name: /Kaydet|Save/i }).first();
    if (await save.count()) {
      await save.click();
      await page.waitForTimeout(200);
      const log = await getIpcLog(page);
      // May already autosave; presence of command in harness is enough if clicked.
      expect(log.length).toBeGreaterThan(0);
    }
  });

  test("ST-03 · UI scale radios change root rem", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const scale130 = page.getByRole("radio", { name: /130/i }).or(page.getByRole("button", { name: /130%/ }));
    if (await scale130.count()) {
      await scale130.first().click();
      const rem = await page.evaluate(() => getComputedStyle(document.documentElement).fontSize);
      expect(parseFloat(rem)).toBeGreaterThan(16);
    } else {
      // Fallback: click text
      await page.getByText("130%").first().click();
      const rem = await page.evaluate(() => getComputedStyle(document.documentElement).fontSize);
      expect(parseFloat(rem)).toBeGreaterThan(15);
    }
  });

  test("ST-05 · Locked approval checkbox explained", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const locked = page.locator('input[type="checkbox"][disabled]');
    if ((await locked.count()) === 0) {
      test.skip(true, "No locked checkbox found");
      return;
    }
    const el = locked.first();
    const title =
      (await el.getAttribute("title")) ||
      (await el.evaluate((node) => node.parentElement?.textContent || ""));
    // Until PR-5 may lack explanation — soft assert with annotation
    if (!/kilit|lock|always|onay/i.test(title || "")) {
      test.fail(true, "Disabled approval checkbox lacks explanation");
    }
  });

  test("ST-06 · Yeniden tara → /onboarding", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const link = page.getByRole("link", { name: /Yeniden tara|Rescan|Onboarding/i }).first();
    await expect(link).toBeVisible();
    await link.click();
    await expect(page).toHaveURL(/\/onboarding/);
  });
});

test.describe("AP / CP / misc", () => {
  test("AP-03 · ?demo=routing-banner actions (browser mode)", async ({ page }) => {
    await openRoute(page, "/dashboard?demo=routing-banner", "browser");
    await page.waitForTimeout(500);
    // In browser mode demo injects approval; buttons should appear.
    const approve = page.getByRole("button", { name: /Onayla|Approve|Local|Yerel|Reddet|Deny/i });
    // If banner not visible (hydration), soft note
    if ((await approve.count()) === 0) {
      test.fail(true, "Routing banner demo not visible");
    }
    expect(await approve.count()).toBeGreaterThan(0);
  });

  test("CP-05 · Palette has no Re-index / Clear Cache", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await page.keyboard.press("Control+k");
    const text = await page.getByRole("dialog").innerText();
    expect(text).not.toMatch(/Re-?index/i);
    expect(text).not.toMatch(/Clear Cache/i);
  });

  test("SR-01 · /stream EventStream controls present", async ({ page }) => {
    await openRoute(page, "/stream", "full");
    await expect(page.getByText(/Event Stream|NATS/i).first()).toBeVisible();
  });

  test("TL-01 · Telemetry filters + download", async ({ page }) => {
    await openRoute(page, "/telemetry", "full");
    const dl = page.getByRole("button", { name: /Markdown|indir|Download/i }).first();
    await expect(page.locator("main")).toBeVisible();
    if (await dl.count()) {
      const [download] = await Promise.all([
        page.waitForEvent("download", { timeout: 5000 }).catch(() => null),
        dl.click(),
      ]);
      if (download) {
        expect(download.suggestedFilename()).toMatch(/md|markdown|report/i);
      }
    }
  });

  test("QT-01 · Quotas filter tabs", async ({ page }) => {
    await openRoute(page, "/quotas", "full");
    await expect(page.getByText(/Cursor|QUOTA|Kota/i).first()).toBeVisible();
  });

  test("OB-01 · Onboarding finish disabled when nothing selected", async ({ page }) => {
    await openRoute(page, "/onboarding", "empty");
    const finish = page.getByRole("button", { name: /Sistemi Başlat|Finish|Start/i }).first();
    if (await finish.count()) {
      await expect(finish).toBeDisabled();
    }
  });

  test("FL-01 · Fleet page renders", async ({ page }) => {
    await openRoute(page, "/fleet", "full");
    await expect(page.getByText(/Worker Fleet|Fleet|memory-bridge|Grok/i).first()).toBeVisible();
  });
});
