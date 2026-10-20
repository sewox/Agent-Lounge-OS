import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { assertLayoutL1toL6 } from "../helpers/layout";
import { openRoute } from "../helpers/nav";
import {
  HEALTH_AGENTS_EMPTY,
  HEALTH_AGENTS_UNMASKED_PATH,
} from "../fixtures/health-agents";

/**
 * Health-UI-2 — runs on D0–D3 (no testIgnore). Layout L1–L6 on every tab;
 * at 1920×1080 (D2) agents PanelGrid shows three inline panels.
 */

const TABS = [
  { tab: "health", path: "/health?tab=health" },
  { tab: "agents", path: "/health?tab=agents" },
  { tab: "dead", path: "/health?tab=dead" },
  { tab: "ignore", path: "/health?tab=ignore" },
] as const;

async function openAgentsWithOverride(
  page: Page,
  override: typeof HEALTH_AGENTS_UNMASKED_PATH | typeof HEALTH_AGENTS_EMPTY | "error",
) {
  await page.addInitScript((payload) => {
    (window as Window & { __QA_LOUNGE_HEALTH__?: unknown }).__QA_LOUNGE_HEALTH__ = payload;
  }, override);
  await openRoute(page, "/health?tab=agents", "full");
}

/** Detail is inline or behind the narrow-mode drawer trigger. */
async function revealDetail(page: Page) {
  const inline = page.locator('[data-panel-slot="detail"]');
  const openBtn = page.locator('[data-qa="open-detail-drawer"]');
  if (!(await inline.isVisible()) && (await openBtn.count()) > 0) {
    await openBtn.click();
  }
  await expect(
    page.locator(
      '[data-panel-slot="detail"], [data-qa="panel-grid-drawer-detail"]',
    ),
  ).toBeVisible();
}

async function revealInspector(page: Page) {
  // Close an open detail drawer first — it intercepts pointer events.
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

async function seedIgnoredRows(page: Page) {
  await page.addInitScript(() => {
    const rows = [
      {
        name: "ignored_fixture_a",
        kind: "unused",
        file: "src/lib/a.ts",
        line: 1,
        detail: "ignored for layout depth",
        project_id: "Agent-Lounge-OS",
        severity: "Potansiyel",
      },
      {
        name: "ignored_fixture_b",
        kind: "broken",
        file: "src/lib/b.ts",
        line: 2,
        detail: "ignored for layout depth",
        project_id: "Agent-Lounge-OS",
        severity: "Kritik",
      },
      {
        name: "ignored_fixture_c",
        kind: "unused",
        file: "src/lib/c.ts",
        line: 3,
        detail: "ignored for layout depth",
        project_id: "EchoMind",
        severity: "Potansiyel",
      },
    ];
    try {
      sessionStorage.setItem("__QA_IGNORED_SYMBOLS__", JSON.stringify(rows));
    } catch {
      /* private mode */
    }
  });
}

test.describe("Health-UI-2 agents + dead", () => {
  for (const { tab, path } of TABS) {
    test(`HU-LAYOUT · L1–L6 on tab=${tab}`, async ({ page }, testInfo) => {
      testInfo.annotations.push({ type: "id", description: `HU-LAYOUT-${tab}` });
      if (tab === "ignore") {
        // Seeded rows for natural list depth (L2); empty ignore is layout-tested separately.
        await seedIgnoredRows(page);
      }
      await openRoute(page, path, "full");
      await expect(page.getByRole("navigation", { name: "Health sections" })).toBeVisible();
      if (tab === "agents") {
        await expect(page.locator('[data-qa-agents-health="ready"]')).toBeVisible();
      }
      const metrics = await assertLayoutL1toL6(page, path, expect);
      expect(metrics.l1_pass).toBe(true);
      expect(metrics.l5_pass).toBe(true);

      if (tab === "agents" && testInfo.project.name === "D2") {
        const grid = page.locator('[data-qa="panel-grid"]');
        await expect(grid).toBeVisible();
        await expect(grid).toHaveAttribute("data-panel-mode", "three");
        await expect(page.locator('[data-panel-slot="list"]')).toBeVisible();
        await expect(page.locator('[data-panel-slot="detail"]')).toBeVisible();
        await expect(page.locator('[data-panel-slot="inspector"]')).toBeVisible();
      }
    });
  }

  test("HU-LAYOUT · L1–L6 on real empty ignore state", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-LAYOUT-ignore-empty" });
    // No seeded rows — honest empty state (no min-h / justify-between spacers).
    await openRoute(page, "/health?tab=ignore", "full");
    await expect(page.locator('[data-qa="dead-symbol-empty"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, "/health?tab=ignore", expect);
    expect(metrics.l1_pass).toBe(true);
    expect(metrics.l2_pass).toBe(true);
    expect(metrics.l5_pass).toBe(true);
  });

  test("HU-01 · agents tab lists all 8 statuses with distinct labels", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-01" });
    await openRoute(page, "/health?tab=agents", "full");
    await expect(page.locator('[data-qa="agents-health-list"]')).toBeVisible();
    const labels = page.locator('[data-qa="agent-status-label"]');
    await expect(labels.first()).toBeVisible();
    const statuses = await page.locator("[data-status]").evaluateAll((els) =>
      [...new Set(els.map((el) => el.getAttribute("data-status")).filter(Boolean))],
    );
    for (const status of [
      "ok",
      "missing",
      "stale",
      "hash_mismatch",
      "non_canonical",
      "unreadable",
      "not_installed",
      "unverified",
    ]) {
      expect(statuses, `missing status ${status}`).toContain(status);
    }
    const unverified = page.locator('[data-qa="agent-status-label"][data-status="unverified"]');
    await expect(unverified.first()).toHaveText(/not verified \(remote url\)|doğrulanmadı \(uzak url\)/i);
    const unverifiedIcon = page.locator(
      '[data-qa="agent-status-icon"][data-icon="link-off"]',
    );
    await expect(unverifiedIcon.first()).toBeVisible();
  });

  test("HU-02 · unverified label ≠ source_unverified approval label", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-02" });
    await openRoute(page, "/health?tab=agents", "full");
    const badgeText = await page
      .locator('[data-qa="agent-status-label"][data-status="unverified"]')
      .first()
      .innerText();
    expect(badgeText).not.toMatch(/Unverified source|Doğrulanmamış kaynak|PENDING_APPROVAL|ONAY_BEKLİYOR/i);
    expect(badgeText).toMatch(/not verified \(remote url\)|doğrulanmadı \(uzak url\)/i);
  });

  test("HU-03 · unmasked fixture paths never leak home dirs in DOM", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-03" });
    await openAgentsWithOverride(page, HEALTH_AGENTS_UNMASKED_PATH);
    await expect(page.locator('[data-qa="agents-health-list"]')).toBeVisible();
    await page.locator('[data-qa="agents-health-row"][data-row-kind="agent"]').first().click();
    await revealDetail(page);
    await revealInspector(page);
    const assertNoHomeLeak = (text: string, label: string) => {
      expect(text, label).not.toMatch(/\/Users\//);
      expect(text, label).not.toMatch(/\/home\//);
      expect(text, label).not.toMatch(/C:\\Users/i);
      expect(text, label).not.toMatch(/C:\/Users/i);
    };
    const main = await page.locator("main").innerText();
    assertNoHomeLeak(main, "main.innerText");
    const raw = await page.locator('[data-qa="agents-health-raw"]').innerText();
    assertNoHomeLeak(raw, "agents-health-raw");
    // Full HTML covers attributes (title/aria) and hidden drawers.
    const html = await page.content();
    assertNoHomeLeak(html, "page.content()");
  });

  test("HU-04 · Şimdi Doğrula only on unverified and only on click", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-04" });
    await openRoute(page, "/health?tab=agents", "full");
    await page
      .locator('[data-qa="agents-health-row"]')
      .filter({ has: page.locator('[data-status="ok"]') })
      .first()
      .click();
    await revealDetail(page);
    await expect(page.locator('[data-qa="agents-probe-now"]')).toHaveCount(0);

    // Close drawer if open so the next selection starts clean on narrow viewports.
    const closeBtn = page.locator('[data-qa="close-panel-drawer"]');
    if (await closeBtn.isVisible()) {
      await closeBtn.click();
    }

    await page
      .locator('[data-qa="agents-health-row"]')
      .filter({ has: page.locator('[data-status="unverified"]') })
      .first()
      .click();
    await revealDetail(page);
    const probeBtn = page.locator('[data-qa="agents-probe-now"]');
    await expect(probeBtn).toBeVisible();
    await expect(page.locator('[data-qa="agents-probe-result"]')).toHaveCount(0);
    await probeBtn.click();
    await expect(page.locator('[data-qa="agents-probe-result"]')).toBeVisible();
  });

  test("HU-05 · empty / error / browser states", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-05" });
    await openAgentsWithOverride(page, HEALTH_AGENTS_EMPTY);
    await expect(page.locator('[data-qa="agents-health-empty-message"]')).toBeVisible();
    await expect(page.locator('[data-qa="agents-health-empty-message"]')).toContainText(
      /Connect|Bağla|lounge-mcp/i,
    );
    await expect(page.locator('[data-qa="agents-empty-connect"]')).toBeVisible();

    await openAgentsWithOverride(page, "error");
    await expect(page.locator('[data-qa-agents-health="error"]')).toBeVisible();
    await expect(page.getByRole("button", { name: /Retry|Yeniden dene/i })).toBeVisible();
  });

  test("CN-01 · Bağla/Onar plan dialog confirm + JSONC consent", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "CN-01" });
    await openRoute(page, "/health?tab=agents", "full");
    await expect(page.locator('[data-qa-agents-health="ready"]')).toBeVisible();
    const metrics = await assertLayoutL1toL6(page, "/health?tab=agents", expect);
    expect(metrics.l1_pass).toBe(true);

    await page
      .locator('[data-qa="agents-health-row"]')
      .filter({ has: page.locator('[data-status="missing"]') })
      .first()
      .click();
    await revealDetail(page);
    const connectBtn = page.locator('[data-qa="agents-connect"]');
    await expect(connectBtn).toBeVisible();
    await connectBtn.click();
    await expect(page.locator('[data-qa="connect-dialog"]')).toBeVisible();
    const planDiff = page.locator('[data-qa="connect-plan-diff"]');
    await expect(planDiff).toBeVisible();
    const planText = await planDiff.innerText();
    // B1: never show sibling secrets or unmasked home paths in the plan dialog.
    expect(planText).not.toMatch(/sk-live-|API_KEY|sibling-secret/i);
    expect(planText).not.toMatch(/\/home\/|\/Users\/|C:\\Users/i);
    expect(planText).toMatch(/\*\*\*/);
    expect(planText).toMatch(/agent-lounge-os|lounge-mcp|~/i);
    const axePlan = await new AxeBuilder({ page }).include('[data-qa="connect-dialog"]').analyze();
    expect(axePlan.violations, JSON.stringify(axePlan.violations, null, 2)).toEqual([]);
    await page.locator('[data-qa="connect-confirm"]').click();
    await expect(page.locator('[data-qa="connect-dialog"]')).toHaveCount(0);

    await page
      .locator('[data-qa="agents-health-row"]')
      .filter({ has: page.locator('[data-status="stale"]') })
      .first()
      .click();
    await revealDetail(page);
    await expect(page.locator('[data-qa="agents-repair"]')).toBeVisible();

    await page.addInitScript(() => {
      (window as Window & { __QA_CONNECT_JSONC_CONSENT__?: boolean }).__QA_CONNECT_JSONC_CONSENT__ =
        true;
    });
    await openRoute(page, "/health?tab=agents", "full");
    await page
      .locator('[data-qa="agents-health-row"]')
      .filter({ has: page.locator('[data-status="missing"]') })
      .first()
      .click();
    await revealDetail(page);
    await page.locator('[data-qa="agents-connect"]').click();
    await expect(page.locator('[data-qa="connect-dialog"][data-mode="jsonc"]')).toBeVisible();
    await page.locator('[data-qa="connect-jsonc-print"]').click();
    const printPayload = page.locator('[data-qa="connect-print-payload"]');
    await expect(printPayload).toBeVisible();
    const printText = await printPayload.innerText();
    // B2: UI print must not expose path_unmasked_for_cli or unmasked homes.
    expect(printText).not.toMatch(/path_unmasked_for_cli/);
    expect(printText).not.toMatch(/\/home\/|\/Users\/|C:\\Users/i);
    expect(printText).toMatch(/~\//);
  });

  test("CN-03 · plan/print DOM never leaks sibling env secret", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "CN-03" });
    // Mock "reads" a sibling MCP server whose env holds this secret; plan IPC must omit it.
    const secret = "sk-live-FAKESECRET-e2e-sibling";
    await page.addInitScript((secretValue) => {
      (window as Window & { __QA_CONNECT_SIBLING_SECRET__?: string }).__QA_CONNECT_SIBLING_SECRET__ =
        secretValue;
    }, secret);
    await openRoute(page, "/health?tab=agents", "full");
    await page
      .locator('[data-qa="agents-health-row"]')
      .filter({ has: page.locator('[data-status="missing"]') })
      .first()
      .click();
    await revealDetail(page);
    await page.locator('[data-qa="agents-connect"]').click();
    await expect(page.locator('[data-qa="connect-dialog"]')).toBeVisible();
    const planText = await page.locator('[data-qa="connect-plan-diff"]').innerText();
    expect(planText).toContain("***");
    expect(planText).toMatch(/agent-lounge-os|~/);
    expect(planText).not.toContain(secret);
    expect(planText).not.toContain("sibling");
    expect(planText).not.toMatch(/\/home\/|\/Users\/|C:\\Users/i);
    const body = await page.locator('[data-qa="connect-dialog"]').innerText();
    expect(body).not.toContain(secret);
    const html = await page.content();
    expect(html).not.toContain(secret);
    // Last plan IPC payload must omit the sibling secret and unmasked homes.
    const payloadJson = await page.evaluate(() =>
      JSON.stringify(
        (window as Window & { __QA_LAST_CONNECT_PAYLOAD__?: unknown }).__QA_LAST_CONNECT_PAYLOAD__ ??
          null,
      ),
    );
    expect(payloadJson).not.toContain(secret);
    expect(payloadJson).not.toContain("sibling");
    expect(payloadJson).not.toMatch(/\/home\/qa-user|\/Users\/|C:\\\\Users/i);
    expect(payloadJson).toContain("***");
    expect(payloadJson).toContain("~/");
  });

  test("CN-02 · Durduruldu note on agents tab when Halted", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "CN-02" });
    await page.addInitScript(() => {
      (window as Window & { __QA_HALT_STATE__?: { halted: boolean } }).__QA_HALT_STATE__ = {
        halted: true,
      };
    });
    await openRoute(page, "/health?tab=agents", "full");
    await expect(page.locator('[data-qa="agents-halted-note"]')).toBeVisible();
    await expect(page.locator('[data-qa="agents-halted-note"]')).toContainText(
      /Halted|Durduruldu/i,
    );
    const metrics = await assertLayoutL1toL6(page, "/health?tab=agents", expect);
    expect(metrics.l1_pass).toBe(true);
    const axe = await new AxeBuilder({ page }).include("main").analyze();
    expect(axe.violations, JSON.stringify(axe.violations, null, 2)).toEqual([]);
  });

  test("HU-05b · non-Tauri browser mode", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-05b" });
    await openRoute(page, "/health?tab=agents", "browser");
    await expect(page.locator('[data-qa-agents-health="browser"]')).toBeVisible();
  });

  test("HU-06 · dead StatsStrip filters + stale warning + keyboard", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-06" });
    await openRoute(page, "/health?tab=dead", "full");
    const strip = page.locator('[data-qa="stats-strip"]');
    await expect(strip).toBeVisible();
    await expect(page.locator('[data-qa="dead-stale-warning"]')).toBeVisible();

    const unused = page.locator('[data-qa="stats-strip-item"][data-stat-id="unused"]');
    await unused.click();
    await expect(unused).toHaveAttribute("aria-pressed", "true");
    const rows = page.locator('[data-qa="dead-symbol-row"]');
    await expect(rows.first()).toBeVisible();
    const count = await rows.count();
    expect(count).toBeGreaterThan(0);
    for (let i = 0; i < count; i++) {
      await expect(rows.nth(i)).toContainText(/unused/i);
    }

    await rows.first().focus();
    await expect(rows.first()).toBeFocused();
  });

  test("HU-07 · axe + badges not colour-alone", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-07" });

    // Scope to main health content (not shell chrome). No disableRules —
    // color-contrast must be real coverage for agents/dead surfaces.
    const runAxe = async (label: string) => {
      const results = await new AxeBuilder({ page }).include("main").analyze();
      expect(
        results.violations,
        `${label}: ${JSON.stringify(results.violations, null, 2)}`,
      ).toEqual([]);
    };

    // Agents: list + detail + inspector (reveal drawers on narrow viewports).
    await openRoute(page, "/health?tab=agents", "full");
    await expect(page.locator('[data-qa="agents-health-list"]')).toBeVisible();
    const pills = page.locator('[data-qa="status-pill"]');
    const n = await pills.count();
    expect(n).toBeGreaterThan(0);
    for (let i = 0; i < n; i++) {
      const text = (await pills.nth(i).innerText()).trim();
      expect(text.length).toBeGreaterThan(0);
    }
    await revealDetail(page);
    await revealInspector(page);
    await runAxe("agents ready");

    // Agents empty + error states.
    await openAgentsWithOverride(page, HEALTH_AGENTS_EMPTY);
    await expect(page.locator('[data-qa="agents-health-empty-message"]')).toBeVisible();
    await runAxe("agents empty");

    await openAgentsWithOverride(page, "error");
    await expect(page.locator('[data-qa-agents-health="error"]')).toBeVisible();
    await runAxe("agents error");

    // Dead tab (stats + list + detail).
    await openRoute(page, "/health?tab=dead", "full");
    await expect(page.locator('[data-qa="stats-strip"]')).toBeVisible();
    await runAxe("dead tab");
  });

  test("HU-08 · agents list keyboard navigation", async ({ page }) => {
    test.info().annotations.push({ type: "id", description: "HU-08" });
    await openRoute(page, "/health?tab=agents", "full");
    await expect(page.locator('[data-qa-agents-health="ready"]')).toBeVisible();
    const list = page.locator('[data-qa="agents-health-list"]');
    await expect(list).toBeVisible();
    const firstId = await page.locator('[data-qa="agents-health-row"]').first().getAttribute("data-row-id");
    await list.focus();
    await expect(list).toBeFocused();
    await page.keyboard.press("ArrowDown");
    const selected = page.locator('[data-qa="agents-health-row"][data-active="true"]');
    await expect(selected).toBeVisible();
    const selectedId = await selected.getAttribute("data-row-id");
    expect(selectedId).not.toBe(firstId);
  });
});
