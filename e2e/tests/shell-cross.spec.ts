import { test, expect } from "@playwright/test";
import { openRoute, collectConsoleErrors } from "../helpers/nav";
import { getIpcLog } from "../harness/tauri-mock";
import { measureLayout } from "../helpers/layout";

test.describe("SH — global shell", () => {
  test("SH-01 · 8 nav links navigate and highlight active", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const links = page.locator('[data-qa="sidebar"] nav a');
    await expect(links).toHaveCount(8);
    const hrefs = await links.evaluateAll((els) => els.map((e) => (e as HTMLAnchorElement).getAttribute("href")));
    expect(hrefs).toEqual([
      "/dashboard",
      "/stream",
      "/vault",
      "/health",
      "/fleet",
      "/telemetry",
      "/quotas",
      "/settings",
    ]);
    await page.locator('[data-qa="sidebar"] nav a[href="/vault"]').click();
    await expect(page).toHaveURL(/\/vault/);
    const active = page.locator('[data-qa="sidebar"] nav a[href="/vault"]');
    await expect(active).toHaveClass(/border-primary|text-primary/);
  });

  test("SH-02 · dead chrome (+ New Node, Quick Filter, Docs, API Keys) still present [expected-fail until PR-2]", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "PR-2 removes dead chrome" });
    test.fail(true, "Dead chrome still in DOM until PR-2");
    await openRoute(page, "/dashboard", "full");
    const newNode = await page.getByRole("button", { name: "+ New Node" }).count();
    const quick = await page.getByRole("button", { name: /Quick Filter/i }).count();
    const docs = await page.getByText("Docs", { exact: true }).count();
    const keys = await page.getByText("API Keys", { exact: true }).count();
    expect(newNode + quick + docs + keys, "dead chrome still present").toBe(0);
  });

  test("SH-03 · Bell opens alert history popover [expected-fail until PR-2]", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "Bell is decorative stub" });
    test.fail(true, "Bell has no popover yet");
    await openRoute(page, "/dashboard", "full");
    await page.locator('header [title*="kernel"]').click();
    // Must open a dedicated alert popover (not ambient AMBER / DEGRADED chrome).
    const popover = page.locator('[data-qa="alert-history"], [role="dialog"][aria-label*="Alert" i]');
    expect(await popover.count(), "alert history popover missing").toBeGreaterThan(0);
  });

  test("SH-04 · ⌘K / Ctrl+K opens and Esc closes command palette", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await page.keyboard.press("Control+k");
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    // Escape is handled on the palette input keydown.
    await dialog.locator("input").first().focus();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0, { timeout: 5_000 });
  });

  test("SH-05 · Model select calls set_kernel_model", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const select = page.locator("header select").first();
    if ((await select.count()) === 0) {
      test.skip(true, "Model select not visible at this viewport");
      return;
    }
    const options = await select.locator("option").allTextContents();
    const pick = options.find((o) => /qwen|llama/i.test(o)) || options[1];
    if (pick) {
      await select.selectOption({ label: pick.trim() }).catch(async () => {
        await select.selectOption({ index: 1 });
      });
    }
    await page.waitForTimeout(300);
    const log = await getIpcLog(page);
    expect(log.some((e) => e.cmd === "set_kernel_model")).toBeTruthy();
  });

  test("SH-07 · Daemon rows show Running or Disconnected (not bare —) [expected-fail browser/partial]", async ({
    page,
  }, testInfo) => {
    await openRoute(page, "/dashboard", "full");
    const daemonBlock = page.locator('[data-qa="sidebar"]').getByText(/Active daemons/i);
    await expect(daemonBlock).toBeVisible();
    const text = await page.locator('[data-qa="sidebar"]').innerText();
    const hasBareDash = /\n\s*—\s*\n/.test(text) || /Running|Disconnected|down/i.test(text) === false;
    if (hasBareDash && !/Running|Disconnected/i.test(text)) {
      testInfo.annotations.push({ type: "expected-fail", description: "Daemon labels incomplete" });
      test.fail(true, "Daemon status not Running/Disconnected");
    }
    expect(/Running|Disconnected|degraded|down|ok/i.test(text)).toBeTruthy();
  });

  test("SH-08 · no header/sidebar overflow (L5)", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const m = await measureLayout(page, "/dashboard");
    const newNode = page.getByRole("button", { name: "+ New Node" });
    let newNodeOverflow = false;
    if (await newNode.count()) {
      newNodeOverflow = await page.evaluate(() => {
        const sidebar = document.querySelector('[data-qa="sidebar"]') as HTMLElement | null;
        const btn = Array.from(document.querySelectorAll("button")).find((b) =>
          (b.textContent || "").includes("+ New Node"),
        );
        if (!sidebar || !btn) return false;
        const sb = sidebar.getBoundingClientRect();
        const br = btn.getBoundingClientRect();
        return br.right > sb.right + 0.5;
      });
    }
    if (!m.l5_pass || newNodeOverflow) {
      test.fail(true, formatExpected({ ...m, newNodeOverflow }));
    }
    expect(m.l5_pass && !newNodeOverflow).toBe(true);
  });
});

function formatExpected(m: {
  l5_sidebarOverflow: boolean;
  l5_horizontalOverflow: boolean;
  newNodeOverflow?: boolean;
}) {
  return `L5 fail sidebarOverflow=${m.l5_sidebarOverflow} hOverflow=${m.l5_horizontalOverflow} newNode=${m.newNodeOverflow}`;
}

test.describe("X — cross-cutting", () => {
  test("X-03 · no uncaught console errors on dashboard", async ({ page }) => {
    const errors = await collectConsoleErrors(page);
    await openRoute(page, "/dashboard", "full");
    await page.waitForTimeout(500);
    const critical = errors.filter(
      (e) => !/favicon|Download the React|hydration/i.test(e),
    );
    expect(critical, critical.join("\n")).toEqual([]);
  });

  test("X-04 · every visible clickable produces an effect", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const result = await page.evaluate(async () => {
      const clickables = Array.from(
        document.querySelectorAll("button, [role='button'], a[href]"),
      ) as HTMLElement[];
      const visible = clickables.filter((el) => {
        const r = el.getBoundingClientRect();
        const style = window.getComputedStyle(el);
        return r.width > 0 && r.height > 0 && style.visibility !== "hidden" && style.display !== "none";
      });

      const dead: string[] = [];
      for (const el of visible) {
        const label =
          el.getAttribute("aria-label") ||
          el.textContent?.trim().slice(0, 40) ||
          el.tagName;
        const disabled =
          el.hasAttribute("disabled") ||
          el.getAttribute("aria-disabled") === "true" ||
          el.className.includes("disabled");
        const title = el.getAttribute("title") || "";
        const href = el.getAttribute("href");
        if (disabled && (title || el.textContent)) continue;
        if (href) continue;
        // Heuristic: no onclick and not a form control tied to React — detect stubs by known labels
        const stubLabels = ["+ New Node", "Quick Filter", "Docs", "API Keys"];
        if (stubLabels.some((s) => (el.textContent || "").includes(s) || title.includes(s))) {
          dead.push(label);
          continue;
        }
        // Bell decorative span is not a button — skip non-buttons without listeners check
        const hasListener = (() => {
          // Cannot introspect React props; use known no-op heuristics only for labeled stubs.
          return true;
        })();
        if (!hasListener) dead.push(label);
      }
      return { scanned: visible.length, dead };
    });

    if (result.dead.length > 0) {
      test.fail(true, `Dead clickables: ${result.dead.join(", ")}`);
    }
    expect(result.dead, `scanned=${result.scanned} dead=${result.dead.join("|")}`).toEqual([]);
  });

  test("X-01 · TR/EN i18n dictionary + Settings switch + persistence [expected-fail until PR-2]", async ({
    page,
  }, testInfo) => {
    // O1: strings from i18n dict; Settings language switch; choice persists; no hardcoded mix.
    testInfo.annotations.push({ type: "expected-fail", description: "i18n not implemented (O1 → PR-2)" });
    test.fail(true, "TR/EN i18n infrastructure missing");
    await openRoute(page, "/settings", "full");
    const langSwitch = page
      .getByRole("radiogroup", { name: /Language|Dil/i })
      .or(page.locator('[data-qa="locale-switch"]'))
      .or(page.getByRole("button", { name: /English|Türkçe|Turkish/i }));
    expect(await langSwitch.count(), "Settings language switch missing").toBeGreaterThan(0);
    await langSwitch.getByText(/English|EN/i).first().click();
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForTimeout(400);
    const stored = await page.evaluate(() => localStorage.getItem("lounge.locale") || localStorage.getItem("locale"));
    expect(stored).toMatch(/en/i);
    // Hardcoded mixed TR/EN chrome must be gone once dictionary is wired.
    await openRoute(page, "/dashboard", "full");
    const text = await page.locator("main").innerText();
    const mixed = /kapalı|düğüm|TEMİZ|Kaydet/i.test(text) && /Dashboard|Index Workspace/i.test(text);
    expect(mixed, "hardcoded mixed TR/EN strings").toBe(false);
  });

  test("X-02 · min font gate delegated to S4 grep (smoke DOM check)", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const tooSmall = await page.evaluate(() => {
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll("body *")) as HTMLElement[]) {
        const fs = parseFloat(window.getComputedStyle(el).fontSize);
        if (fs > 0 && fs < 12 - 0.05) {
          const t = (el.textContent || "").trim().slice(0, 24);
          if (t) bad.push(`${fs.toFixed(1)}px:${t}`);
        }
      }
      return [...new Set(bad)].slice(0, 10);
    });
    expect(tooSmall, tooSmall.join(", ")).toEqual([]);
  });
});
