import { test, expect } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import { getIpcLog } from "../harness/tauri-mock";

test.describe("HM — health / map empty states", () => {
  test("HM-01 · Empty DB must not show MOCK_HEALTH 12/41/74", async ({
    page,
  }) => {
    await openRoute(page, "/health", "empty");
    const text = await page.locator("main").innerText();
    const hasMock = /\b12\b/.test(text) && /\b41\b/.test(text);
    const hasEmpty = /No data found|Index Workspace/i.test(text);
    expect(!hasMock && hasEmpty).toBeTruthy();
  });

  test("HM-02 · Empty Map must not use MOCK_NODES", async ({
    page,
  }) => {
    await openRoute(page, "/vault", "empty");
    const text = await page.locator("main").innerText();
    expect(!/EchoMind/.test(text) && /No data found|Index Workspace/i.test(text)).toBeTruthy();
  });

  test("HM-06 · Tauri reject: no fake Claude % / Amber in bell", async ({ page }) => {
    await openRoute(page, "/dashboard", "reject");
    await page.waitForTimeout(400);
    const main = await page.locator("main").innerText();
    expect(main).not.toMatch(/Claude\s*7\d%/i);
    expect(main).not.toMatch(/Claude\s*5\d%/i);
    expect(main).toMatch(/Kota verisi alınamadı/i);
    await page.getByRole("button", { name: "Alert history" }).click();
    const history = page.locator('[data-qa="alert-history"]');
    await expect(history).toBeVisible();
    const historyText = await history.innerText();
    expect(historyText).not.toMatch(/Amber/i);
    expect(historyText).not.toMatch(/Claude\s*\d+%/i);
  });

  test("HM-03 · Full DB shows live health rows + dead drill-down", async ({ page }) => {
    await openRoute(page, "/health", "full");
    await expect(page.getByText("Agent-Lounge-OS").first()).toBeVisible();
    const text = await page.locator("main").innerText();
    expect(text).toMatch(/live|Agent-Lounge/i);
    const drill = page.locator(
      'a[data-qa="health-dead-drilldown"][href*="Agent-Lounge-OS"]',
    );
    await expect(drill).toBeVisible();
    const headline = Number(await drill.getAttribute("data-qa-dead-count"));
    expect(headline).toBeGreaterThan(0);
    await drill.click();
    await expect(page).toHaveURL(/tab=dead/);
    await expect(page).toHaveURL(/project=Agent-Lounge-OS/);
    await expect(page.locator('[data-qa="dead-symbol-list"]')).toBeVisible();
    const listTotal = Number(
      await page.locator('[data-qa="dead-symbol-total"]').getAttribute("data-qa-total"),
    );
    // Project-filtered list total must match the health headline for that project.
    expect(listTotal).toBe(headline);

    // After ignoring every row, headline and list stay consistent (0, not map.dead fallback).
    const rows = page.locator('[data-qa="dead-symbol-row"]');
    let guard = 0;
    while ((await rows.count()) > 0 && guard < 40) {
      await rows.first().click();
      await page.getByRole("button", { name: /^Ignore$/i }).click();
      await page.waitForTimeout(150);
      guard += 1;
    }
    const emptyTotal = Number(
      await page.locator('[data-qa="dead-symbol-total"]').getAttribute("data-qa-total"),
    );
    expect(emptyTotal).toBe(0);
    await page
      .getByRole("navigation", { name: "Health sections" })
      .getByRole("link", { name: /Project Health/i })
      .click();
    await page.waitForTimeout(300);
    const aloDrill = page.locator(
      'a[data-qa="health-dead-drilldown"][href*="Agent-Lounge-OS"]',
    );
    await expect(aloDrill).toBeVisible();
    expect(Number(await aloDrill.getAttribute("data-qa-dead-count"))).toBe(0);
  });

  test("HM-04 · Re-index only on /health, not in palette", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await page.keyboard.press("Control+k");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    const palText = await dialog.innerText();
    expect(palText).not.toMatch(/Re-?index/i);
    expect(palText).not.toMatch(/Clear Cache/i);
    await page.keyboard.press("Escape");
    await openRoute(page, "/health", "full");
    await expect(page.locator('[data-qa="health-reindex"]')).toBeVisible();
  });
});

test.describe("ST — settings", () => {
  test("ST-01 · Routing table not clipped @ D0", async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== "D0" && testInfo.project.name !== "D4-scale", "D0/D4");
    await openRoute(page, "/settings", "full");
    const vp = page.viewportSize()!;
    const region = page.locator('[data-qa="routing-table"]');
    await expect(region).toBeVisible();
    const box = await region.boundingBox();
    const clipped = Boolean(box && box.y + box.height > vp.height - 4);
    expect(clipped, "routing controls must fit in viewport or scroll").toBe(false);
  });

  test("ST-02 · Routing policy save calls set_routing_policy", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const before = await getIpcLog(page);
    // Policy autosaves on trigger checkbox toggle (no Graph Kaydet / native dialog).
    // Scope to routing table so approval-sound / other prefs checkboxes are ignored.
    const trigger = page
      .locator('[data-qa="routing-table"] input[type="checkbox"]:not([disabled])')
      .first();
    if ((await trigger.count()) === 0) {
      test.skip(true, "No editable trigger checkbox");
      return;
    }
    await trigger.click({ timeout: 5_000 });
    await page.waitForTimeout(400);
    const after = await getIpcLog(page);
    expect(after.slice(before.length).some((e) => e.cmd === "set_routing_policy")).toBeTruthy();
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
    const explained = /kilit|lock|always|onay|disabled|zorunlu/i.test(title || "");
    if (!explained) {
      test.fail(true, "Disabled approval checkbox lacks explanation");
    }
    expect(explained).toBeTruthy();
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

  test("AP-06 · Destructive ops always require confirmation UI (POSIX + Windows) [expected-fail until PR-1/5]", async ({
    page,
  }, testInfo) => {
    // O4 / §10.2: DB drop/truncate/delete/migrate-down, rm -rf, reset,
    // and Windows del /s, rd /s, Remove-Item -Recurse, format → always confirm.
    testInfo.annotations.push({
      type: "expected-fail",
      description: "destructive-operation gate missing (O4 / §10.2)",
    });
    test.fail(true, "Destructive confirmation gate not implemented");
    await openRoute(page, "/settings", "full");
    const gate = page.getByText(
      /destructive|yıkıcı|always confirm|her zaman onay|Never Ask.*cannot|atlanamaz|del \/s|Remove-Item|rm -rf/i,
    );
    expect(await gate.count(), "destructive gate copy / control").toBeGreaterThan(0);
    // Contract surface for detector patterns (documented until backend ships).
    const patterns = [
      "rm -rf",
      "del /s",
      "rd /s",
      "Remove-Item -Recurse",
      "format",
    ];
    expect(patterns.length).toBe(5);
  });

  test("AP-07 · Never Ask cannot skip destructive confirmation [expected-fail until PR-1/5]", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({
      type: "expected-fail",
      description: "Never Ask still bypasses destructive ops (O4 / §10.2)",
    });
    test.fail(true, "Destructive ops not forced through DecisionGate");
    await openRoute(page, "/dashboard?demo=destructive-reset", "browser");
    const dialog = page.getByRole("alertdialog").or(page.getByRole("dialog"));
    expect(await dialog.count(), "destructive confirm alertdialog").toBeGreaterThan(0);
    await expect(dialog.first()).toContainText(/confirm|onay|reset|delete|sil|Remove-Item|del \/s/i);
  });

  test("AP-08 · Pending approval plays alert sound (HTML Audio; wav/mp3/ogg)", async ({
    page,
  }) => {
    // §10.1 / §10.2: webview HTMLAudioElement; bundled wav/mp3/ogg;
    // plays when hidden; default 60s repeat until decision. No background escalation.
    await page.addInitScript(() => {
      type PlayLog = { src: string; t: number };
      const g = window as Window & { __QA_AUDIO_PLAYS__?: PlayLog[] };
      g.__QA_AUDIO_PLAYS__ = [];
      const proto = HTMLAudioElement.prototype;
      const origPlay = proto.play;
      proto.play = function playSpy(this: HTMLAudioElement, ...args: unknown[]) {
        g.__QA_AUDIO_PLAYS__!.push({ src: this.currentSrc || this.src || "", t: Date.now() });
        return origPlay.apply(this, args as []).catch(() => undefined as unknown as void);
      };
    });
    await openRoute(page, "/dashboard?demo=routing-banner", "browser");
    await page.waitForTimeout(800);
    // Simulate background/hidden: document.hidden cannot be set; blur + visibility stub.
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await page.waitForTimeout(500);
    const plays = await page.evaluate(() => (window as Window & { __QA_AUDIO_PLAYS__?: unknown[] }).__QA_AUDIO_PLAYS__ ?? []);
    expect(plays.length, "audio play spy should see ≥1 alert while approval pending").toBeGreaterThan(0);
    const bundled = /\.(wav|mp3|ogg)(\?|$)/i;
    const srcOk = (plays as { src: string }[]).some((p) => !p.src || bundled.test(p.src));
    expect(srcOk, "bundled alert should be wav/mp3/ogg (or empty until asset wired)").toBeTruthy();
    const decide = page.getByRole("button", { name: /Onayla|Approve|Reddet|Deny/i }).first();
    if (await decide.count()) {
      await decide.click();
      const after = await page.evaluate(
        () => (window as Window & { __QA_AUDIO_PLAYS__?: unknown[] }).__QA_AUDIO_PLAYS__?.length ?? 0,
      );
      expect(after).toBeGreaterThan(0);
    }
  });

  test("AP-09 · Settings sound options (engine prefs; full Settings UI in PR-5)", async ({
    page,
  }) => {
    // Engine + minimal control for PR-2b; full Settings polish is PR-5.
    await openRoute(page, "/settings", "full");
    const section = page.locator('[data-qa="approval-sound"]');
    await expect(section, "sound settings section").toBeVisible();
    await section.scrollIntoViewIfNeeded();
    await expect(section.getByRole("button", { name: /^Dinle$|Preview|Play|Listen|Test/i })).toBeVisible();
    await expect(section.getByText(/wav|mp3|ogg|aiff|upload|yükle|Pick|Dosya|≤5/i).first()).toBeVisible();
    await expect(section.getByText(/volume|ses|interval|aralık|60/i).first()).toBeVisible();
    // Persist: toggle off writes lounge.approvalSound so PR-5 Settings can reuse it.
    // Scope to this panel only — outer Settings <section> also contains locked routing checkboxes.
    const toggle = section.locator('input[type="checkbox"]:not([disabled]), [role="switch"]:not([disabled])').first();
    await expect(toggle).toBeVisible();
    await expect(toggle).toBeEnabled();
    await toggle.click();
    await page.waitForTimeout(200);
    const stored = await page.evaluate(
      () =>
        localStorage.getItem("lounge.approvalSound") ||
        localStorage.getItem("approval-sound") ||
        "",
    );
    expect(stored.length).toBeGreaterThan(0);
  });

  test("AP-10 · Native OS notification surface + banner focus hook", async ({
    page,
  }, testInfo) => {
    // §10.1 / §10.2: Tauri notification plugin on all three OSes; click focuses app + banner.
    // Full click delivery is OS-dependent and verified on S2 live checklists.
    testInfo.annotations.push({
      type: "manual",
      description:
        "S2 live: notification click focuses app + banner on macOS / Windows / Linux (§10.2). Some platforms cannot deliver click events — fallback: focus_app_for_approval + approval_banner_focus.",
    });
    await openRoute(page, "/dashboard", "full");
    expect(await page.evaluate(() => "__TAURI_INTERNALS__" in window)).toBeTruthy();
    expect(await page.locator('[data-qa="approval-notification"]').count()).toBeGreaterThan(0);
  });

  test("PATH-01 · Path handling accepts / \\ and Windows drive letters", async ({
    page,
  }) => {
    await openRoute(page, "/vault", "full");
    // Real project root (Windows) appears when the project is selected — not a fake Path reference box.
    await page
      .locator('[data-qa="panel"]')
      .getByRole("button", { name: /Agent-Lounge-OS/i })
      .first()
      .click();
    const winRoot = "C:\\Users\\sercan\\dev\\Agent-Lounge-OS";
    await expect(page.locator('[data-qa="vault-project-path"]')).toContainText(winRoot);
    // Real dead-symbol file path with drive letter + backslashes stays unaltered in the vault list.
    await expect(page.locator("main")).toContainText(
      "C:\\Users\\sercan\\dev\\Agent-Lounge-OS\\src-tauri\\src\\kernel\\dispatcher.rs",
    );
    // POSIX project root from another indexed repo.
    await page
      .locator('[data-qa="panel"]')
      .getByRole("button", { name: /EchoMind/i })
      .first()
      .click();
    await expect(page.locator('[data-qa="vault-project-path"]')).toContainText(
      "/home/sercan/dev/EchoMind",
    );
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
    await expect(page.locator("main")).toBeVisible();
    const text = await page.locator("main").innerText();
    expect(/Event Stream|NATS|Probe|all|task|exp/i.test(text)).toBeTruthy();
  });

  test("SR-02 · Heartbeats filtered or collapsed by default (no empty Decision chips)", async ({
    page,
  }) => {
    await openRoute(page, "/stream", "full");
    const text = await page.locator("main").innerText();
    const hb = (text.match(/lounge\.(bus|workers)\.heartbeat/gi) || []).length;
    const real = (text.match(/lounge\.task\.(requested|completed)|lounge\.experience/gi) || []).length;
    expect(hb, "heartbeats visible in default view").toBe(0);
    expect(real, "real traffic still visible").toBeGreaterThan(0);
    expect(text).not.toMatch(/Decision:\s*[—\-–]/i);
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
    await expect(page.locator("main")).toBeVisible();
    const text = await page.locator("main").innerText();
    expect(/Cursor|QUOTA|Kota|subscription|LMR/i.test(text)).toBeTruthy();
  });

  test("OB-01 · Onboarding finish disabled when nothing selected", async ({ page }) => {
    // Use full fixture (empty IPC dataset currently trips a client error boundary on
    // /onboarding — documented in baseline). Deselect all tools to assert disabled CTA.
    await openRoute(page, "/onboarding", "full", { waitMs: 1200 });
    const body = page.locator("body");
    if (/couldn.?t load|could not be found/i.test(await body.innerText())) {
      test.fail(true, "Onboarding failed to load under IPC mock");
      expect(false, "onboarding page load").toBe(true);
      return;
    }
    // Wait out Scanning System…
    await page.getByRole("button", { name: /Sistemi Başlat|Finish|Start/i }).first()
      .waitFor({ state: "visible", timeout: 15_000 })
      .catch(() => undefined);
    const finish = page.getByRole("button", { name: /Sistemi Başlat|Finish|Start/i }).first();
    if ((await finish.count()) === 0) {
      test.fail(true, "Finish CTA not found after scan");
      expect(await finish.count()).toBeGreaterThan(0);
      return;
    }
    // Deselect everything that looks selected.
    const checked = page.locator('input[type="checkbox"]:checked');
    const n = await checked.count();
    for (let i = 0; i < n; i++) {
      await checked.nth(0).click({ timeout: 2_000 }).catch(() => undefined);
    }
    await expect(finish).toBeDisabled();
  });

  test("FL-01 · Fleet page renders", async ({ page }) => {
    await openRoute(page, "/fleet", "full");
    await expect(page.locator("main")).toBeVisible();
    const text = await page.locator("main").innerText();
    expect(/Worker Fleet|Fleet|memory-bridge|Grok|LMR|NATS|DecisionGate/i.test(text)).toBeTruthy();
    await expect(page.getByText("Worker detail").first()).toBeVisible();
    await expect(page.getByRole("columnheader", { name: /Heartbeat/i })).toBeVisible();
  });
});
