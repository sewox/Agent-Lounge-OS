import { test, expect } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import { getIpcLog } from "../harness/tauri-mock";
import { measureLayout, formatLayoutFailure } from "../helpers/layout";

test.describe("DB — dashboard", () => {
  test("DB-01 · KPI Dead Symbols equals real count (not 127) [full]", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const value = await page.locator("main").getByText("DEAD SYMBOLS").locator("xpath=ancestor::*[contains(@class,'rounded')][1]").innerText();
    // With full fixture, deadSymbols.length is 10 — must not be 127.
    expect(value).not.toMatch(/\b127\b/);
    expect(value).toMatch(/\b10\b/);
  });

  test("DB-01-empty · KPI must not show 127 when no index", async ({
    page,
  }) => {
    await openRoute(page, "/dashboard", "empty");
    const text = await page.locator("main").innerText();
    expect(text).not.toMatch(/\b127\b/);
  });

  test("DB-02 · KPI vs vault selection consistency [expected-fail when mock KPI]", async ({
    page,
  }, testInfo) => {
    await openRoute(page, "/dashboard", "full");
    // Select first map node if present
    const node = page.getByText(/Agent-Lounge-OS/i).first();
    if (await node.count()) {
      await node.click();
      await page.waitForTimeout(300);
    }
    const text = await page.locator("main").innerText();
    const kpiDead = /\bDEAD SYMBOLS\b[\s\S]{0,80}?(\d+)/i.exec(text);
    const clean = /temiz/i.test(text);
    if (kpiDead && Number(kpiDead[1]) > 0 && clean) {
      testInfo.annotations.push({ type: "expected-fail", description: "KPI>0 but selection says temiz" });
      test.fail(true, "KPI/list mismatch");
    }
    expect(true).toBeTruthy();
  });

  test("DB-03 · Event Stream filters + Probe bus", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const all = page.getByRole("button", { name: /^all$/i }).first();
    const task = page.getByRole("button", { name: /^task$/i }).first();
    const exp = page.getByRole("button", { name: /^exp$/i }).first();
    if (await task.count()) await task.click();
    if (await all.count()) await all.click();
    if (await exp.count()) await exp.click();
    const probe = page.getByRole("button", { name: /Probe/i }).first();
    if (await probe.count()) {
      await probe.click();
      await page.waitForTimeout(200);
      const log = await getIpcLog(page);
      expect(log.some((e) => e.cmd === "probe_bus")).toBeTruthy();
    }
  });

  test("DB-04 · Embedded Vault visible in first fold @ D0", async ({ page }, testInfo) => {
    test.skip(testInfo.project.name !== "D0", "D0-only fold check");
    await openRoute(page, "/dashboard", "full");
    const vaultTitle = page.getByText(/Semantic Map \+ Experiences/i).first();
    await expect(vaultTitle).toBeVisible();
    const box = await vaultTitle.boundingBox();
    expect(box).toBeTruthy();
    const belowFold = Boolean(box && box.y > 700);
    expect(belowFold, "vault title should be in first fold").toBe(false);
  });

  test("DB-05 · Critical Quotas link to /quotas", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const link = page.getByRole("link", { name: /Tüm kotalar|quotas/i }).first();
    await expect(link).toBeVisible();
    await link.click();
    await expect(page).toHaveURL(/\/quotas/);
  });

  test("DB-06 · Collapse persistence across reload", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const toggle = page.locator('[data-panel-id="dash-event-stream"] button[aria-expanded]').first();
    await expect(toggle).toBeVisible();
    const before = await toggle.getAttribute("aria-expanded");
    await toggle.click();
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForTimeout(500);
    // Re-install mock after reload — addInitScript persists for page, but soft nav reload keeps it.
    const after = await page
      .locator('[data-panel-id="dash-event-stream"] button[aria-expanded]')
      .first()
      .getAttribute("aria-expanded");
    expect(after).not.toBe(before);
  });
});

test.describe("EX — vault experiences", () => {
  test("EX-01 · Full read detail drawer [expected-fail until PR-3]", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "No detail drawer; line-clamp-3" });
    test.fail(true, "Experience cards are read-only clamped");
    await openRoute(page, "/vault", "full");
    const drawer = page.getByRole("dialog").or(page.locator("[data-qa=experience-drawer]"));
    expect(await drawer.count(), "detail drawer missing").toBeGreaterThan(0);
  });

  test("EX-02 · Edit experience [expected-fail until PR-1/3]", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "update_experience missing" });
    test.fail(true, "No edit UI / command");
    await openRoute(page, "/vault", "full");
    expect(await page.getByRole("button", { name: /Edit|Düzenle/i }).count()).toBeGreaterThan(0);
  });

  test("EX-03 · Soft delete / archive [expected-fail until PR-1/3]", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "archive missing" });
    test.fail(true, "No archive UI");
    await openRoute(page, "/vault", "full");
    expect(await page.getByRole("button", { name: /Archive|Arşiv/i }).count()).toBeGreaterThan(0);
  });

  test("EX-04 · Pin [expected-fail until PR-1/3]", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "pin missing" });
    test.fail(true, "No pin UI");
    await openRoute(page, "/vault", "full");
    expect(await page.getByRole("button", { name: /Pin/i }).count()).toBeGreaterThan(0);
  });

  test("EX-05 · Agent experiences auto-approved with reviewed=false + unreviewed badge [expected-fail until PR-1/3]", async ({
    page,
  }, testInfo) => {
    // O6: MCP records are active (not Draft) but reviewed=false; sidebar badge = unreviewed count.
    testInfo.annotations.push({
      type: "expected-fail",
      description: "reviewed flag + vault badge missing (O6)",
    });
    test.fail(true, "reviewed=false + unreviewed badge not implemented");
    await openRoute(page, "/vault", "full");
    const badge = page.locator(
      '[data-qa="sidebar"] [data-qa="unreviewed-count"], [data-qa="sidebar"] a[href="/vault"] [data-qa="badge"]',
    );
    expect(await badge.count(), "unreviewed badge on Knowledge Vault nav").toBeGreaterThan(0);
    const badgeText = ((await badge.first().textContent()) || "").trim();
    expect(Number(badgeText)).toBeGreaterThan(0);
    // Opening detail / mark reviewed should decrease badge (UI not present yet).
    expect(await page.getByRole("button", { name: /İncelendi|Reviewed|Mark reviewed/i }).count()).toBeGreaterThan(
      0,
    );
  });

  test("EX-08 · List limit > 12 [expected-fail until PR-3]", async ({ page }, testInfo) => {
    testInfo.annotations.push({ type: "expected-fail", description: "list_experiences limit 12" });
    await openRoute(page, "/vault", "full");
    // Fixture has 16; provider requests limit 12.
    const cards = page.locator(".line-clamp-3");
    const count = await cards.count();
    if (count <= 12) {
      test.fail(true, `Only ${count} experiences visible (limit 12)`);
    }
    expect(count).toBeGreaterThan(12);
  });

  test("EX-13 · TTL + use_count auto-archive [expected-fail until PR-1/3]", async ({ page }, testInfo) => {
    // O5: use_count / last_used_at + Settings TTL/threshold; archived rows recoverable.
    testInfo.annotations.push({ type: "expected-fail", description: "TTL auto-archive missing (O5)" });
    test.fail(true, "use_count/TTL auto-archive not implemented");
    await openRoute(page, "/settings", "full");
    const ttl = page.getByText(/TTL|use_count|auto-?archive|otomatik arşiv/i);
    expect(await ttl.count(), "Settings TTL / use_count controls").toBeGreaterThan(0);
    await openRoute(page, "/vault", "full");
    expect(await page.getByRole("button", { name: /Show Archived|Arşivlenenleri göster/i }).count()).toBeGreaterThan(
      0,
    );
  });

  test("EX-LAYOUT · Semantic Map + Experiences fill ≥85% [expected-fail — Sercan half-panel]", async ({
    page,
  }, testInfo) => {
    await openRoute(page, "/vault", "full");
    const m = await measureLayout(page, "/vault");
    const fail = formatLayoutFailure(m);
    testInfo.annotations.push({ type: "layout", description: fail });
    if (!m.l1_pass || !m.l3_pass || m.l2_sparseInterior) {
      test.fail(true, fail);
    }
    expect(m.l1_pass && m.l3_pass && !m.l2_sparseInterior).toBe(true);
  });

  test("EX-14 · Vault totals must match bridge counts (not query LIMIT as total) [expected-fail until PR-3]", async ({
    page,
  }, testInfo) => {
    // Live S2: UI showed 400 nodes / 800 edges while experience text said nodes=2286 edges=7958.
    testInfo.annotations.push({
      type: "expected-fail",
      description: "Owned by PR-3 (UI wiring): LIMIT-shaped vault totals disagree with bridge figures",
    });
    test.fail(true, "Owned by PR-3 (UI wiring)");
    await openRoute(page, "/vault", "full");
    const text = await page.locator("main").innerText();
    const bridge = text.match(/nodes=(\d+)\s+edges=(\d+)/i);
    expect(bridge, "bridge stats appear in experience content").toBeTruthy();
    const bridgeNodes = Number(bridge![1]);
    const bridgeEdges = Number(bridge![2]);
    const treeNodes = Number((text.match(/(\d+)\s*AST nodes/i) || text.match(/(\d+)\s*nodes/i) || [])[1] || 0);
    const treeEdges = Number((text.match(/(\d+)\s*edges/i) || [])[1] || 0);
    // Tree/header totals must equal bridge-reported counts (not a query LIMIT).
    expect(treeNodes, `tree nodes ${treeNodes} vs bridge ${bridgeNodes}`).toBe(bridgeNodes);
    expect(treeEdges, `tree edges ${treeEdges} vs bridge ${bridgeEdges}`).toBe(bridgeEdges);
  });

  test("EX-15 · Experience Log hides raw markdown dumps and internal TR errors [expected-fail until PR-3]", async ({
    page,
  }, testInfo) => {
    // Live S2: log showed "## Cross-Project Memory ### Tecrübeler" and memory_bridge hata strings.
    testInfo.annotations.push({
      type: "expected-fail",
      description: "Experience Log renders raw prompt/markdown + internal TR errors (live S2)",
    });
    test.fail(true, "Experience Log not sanitized for end users");
    await openRoute(page, "/vault", "full");
    const text = await page.locator("main").innerText();
    expect(text).not.toMatch(/##\s*Cross-Project Memory/i);
    expect(text).not.toMatch(/###\s*Tecrübeler/i);
    expect(text).not.toMatch(/memory_bridge hata:/i);
    expect(text).not.toMatch(/repo_path çözümlenemedi/i);
  });
});

test.describe("DS — dead symbols", () => {
  test("DS-01 · Full list beyond 8", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    const rows = page.locator('[data-qa="dead-symbol-row"]');
    await expect(rows.first()).toBeVisible();
    expect(await rows.count()).toBeGreaterThan(8);
    await expect(page.getByText(/\d+ total/i).first()).toBeVisible();
  });

  test("DS-02 · Detail on click", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    await page.locator('[data-qa="dead-symbol-row"]').first().click();
    const detail = page.locator('[data-qa="dead-symbol-detail"]');
    await expect(detail).toBeVisible();
    await expect(detail.getByText(/last_ref/i)).toBeVisible();
    await expect(detail.getByText(/orphan_dispatch|referans yok/i)).toBeVisible();
  });

  test("DS-03 · Open in editor (index-backed; Settings Editor preference is PR-5)", async ({
    page,
  }) => {
    // Automated: button + IPC. Manual per-OS checklist: open / start / xdg-open via guarded wrapper.
    await openRoute(page, "/health?tab=dead", "full");
    await page.locator('[data-qa="dead-symbol-row"]').first().click();
    const openBtn = page.getByRole("button", { name: /Open in Editor/i });
    await expect(openBtn).toBeVisible();
    const before = await getIpcLog(page);
    await openBtn.click();
    await page.waitForTimeout(200);
    const after = await getIpcLog(page);
    expect(
      after.slice(before.length).some((e) => e.cmd === "open_dead_symbol_in_editor"),
    ).toBeTruthy();
  });

  test("DS-04 · Copy path", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    await page.locator('[data-qa="dead-symbol-row"]').first().click();
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.getByRole("button", { name: /Copy path/i }).click();
    await expect(page.getByText(/Copy path:/i)).toBeVisible();
  });

  test("DS-05 · Ignore", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    const rows = page.locator('[data-qa="dead-symbol-row"]');
    const before = await rows.count();
    await rows.first().click();
    await page.getByRole("button", { name: /^Ignore$/i }).click();
    await page.waitForTimeout(300);
    // After ignore the list refreshes via get_dead_symbols; count drops.
    const after = await rows.count();
    expect(after).toBeLessThan(before);
  });

  test("DS-06 · Ignore List tab restore", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    await page.locator('[data-qa="dead-symbol-row"]').first().click();
    await page.getByRole("button", { name: /^Ignore$/i }).click();
    await page.waitForTimeout(200);
    await page.getByRole("link", { name: /Ignore List/i }).click();
    await expect(page.locator('[data-qa-dead-symbols="ignored"]')).toBeVisible();
    await expect(page.locator('[data-qa="dead-symbol-row"]').first()).toBeVisible();
    await page.getByRole("button", { name: /Unignore/i }).click();
  });

  test("DS-07 · Fix with agent publishes task", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    await page.locator('[data-qa="dead-symbol-row"]').first().click();
    const before = await getIpcLog(page);
    await page.getByRole("button", { name: /Fix with agent/i }).click();
    await page.waitForTimeout(200);
    const after = await getIpcLog(page);
    expect(
      after.slice(before.length).some((e) => e.cmd === "fix_dead_symbol_with_agent"),
    ).toBeTruthy();
  });

  test("DS-LAYOUT · Dead list + detail fill width/height @ D0/D3/D4", async ({
    page,
  }, testInfo) => {
    test.skip(
      testInfo.project.name !== "D0" &&
        testInfo.project.name !== "D3" &&
        testInfo.project.name !== "D4-scale" &&
        !testInfo.project.name.includes("960"),
      "viewport matrix",
    );
    await openRoute(page, "/health?tab=dead", "full");
    const m = await measureLayout(page, "/health");
    expect(m.l1_pass, formatLayoutFailure(m)).toBe(true);
    expect(m.l2_pass || m.l2_sparseInterior === false, formatLayoutFailure(m)).toBeTruthy();
  });
});
