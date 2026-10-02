import { test, expect } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import { getIpcLog } from "../harness/tauri-mock";
import { measureLayout, formatLayoutFailure } from "../helpers/layout";

async function openVaultProject(page: import("@playwright/test").Page, name: string) {
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

  test("DB-02 · KPI vs vault selection consistency", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    // Prefer Semantic Map project row — avoid experience cards that also mention the name.
    const node = page.locator('[data-qa="vault-project-row"]').filter({ hasText: /Agent-Lounge-OS/i }).first();
    if (await node.count()) {
      await node.click();
      await page.waitForTimeout(300);
    }
    const text = await page.locator("main").innerText();
    const kpiDead = /\bDEAD SYMBOLS\b[\s\S]{0,80}?(\d+)/i.exec(text);
    const clean = /temiz/i.test(text);
    // When KPI reports dead > 0, selection must not claim "temiz".
    if (kpiDead && Number(kpiDead[1]) > 0) {
      expect(clean, "KPI>0 must not show temiz for selection").toBe(false);
    }
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

  test("DB-04 · Embedded Vault visible in first fold", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    const vaultTitle = page.getByText(/Semantic Map \+ Experiences/i).first();
    await expect(vaultTitle).toBeVisible();
    const box = await vaultTitle.boundingBox();
    expect(box, "vault title must have a layout box").toBeTruthy();
    const vp = page.viewportSize()!;
    // Tightened vs prior 1.15× everywhere: desktop widths must be true first-fold.
    // D960 (≤960px) stacks KPI + stream above the vault, so allow one short scroll
    // (still stricter than unbounded / endless scroll).
    const maxTop = vp.width <= 960 ? vp.height * 1.15 : vp.height;
    expect(box!.y, "vault title top must not be above the page").toBeGreaterThanOrEqual(-2);
    expect(box!.y, "vault title must stay near the first fold").toBeLessThan(maxTop);
    if (vp.width > 960) {
      expect(
        box!.y + Math.min(box!.height, 24),
        "vault title text must intersect the first viewport on desktop",
      ).toBeLessThanOrEqual(vp.height);
    }
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
  test("EX-01 · Full read detail drawer", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    const card = page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /Indexed dispatcher\.rs/i })
      .first();
    await expect(card).toBeVisible();
    await card.click();
    const drawer = page.locator("[data-qa=experience-drawer]");
    await expect(drawer).toBeVisible();
    const text = await drawer.innerText();
    expect(text).toMatch(/Indexed dispatcher\.rs \+ NATS subjects/i);
    expect(text).not.toMatch(/line-clamp/i);
  });

  test("EX-02 · Edit experience", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    await page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /Indexed dispatcher\.rs/i })
      .first()
      .click();
    const drawer = page.locator("[data-qa=experience-drawer]");
    await expect(drawer).toBeVisible();
    await drawer.getByRole("button", { name: /^Edit$/i }).click();
    await expect(drawer).toHaveAttribute("data-mode", "edit");
    const adrInput = drawer.locator('[data-qa="experience-adr-input"]');
    await expect(adrInput).toBeVisible();
    await expect(adrInput).toHaveValue(/Indexed dispatcher\.rs/i);
    await adrInput.fill("Edited ADR summary for e2e verification.");
    await drawer.getByRole("button", { name: /^Save$/i }).click();
    await expect(drawer).toContainText("Edited ADR summary for e2e verification.");
    const log = await getIpcLog(page);
    const update = log.find((entry) => entry.cmd === "update_experience");
    expect(update).toBeTruthy();
    expect(JSON.stringify(update?.args ?? {})).toMatch(/Edited ADR summary for e2e verification/);
  });

  test("EX-03 · Soft delete / archive", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "EchoMind");
    const target = page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /Experience ADR #14/i })
      .first();
    await expect(target).toBeVisible();
    await target.click();
    const drawer = page.locator("[data-qa=experience-drawer]");
    await expect(drawer).toBeVisible();
    await drawer.getByRole("button", { name: /^Archive$/i }).click();
    await expect(page.locator('[data-qa="archive-confirm"]')).toBeVisible();
    await page.locator('[data-qa="archive-confirm"]').getByRole("button", { name: /^Archive$/i }).click();
    await expect(drawer).toHaveCount(0);
    await expect(
      page.locator('[data-qa="experience-card"]').filter({ hasText: /Experience ADR #14/i }),
    ).toHaveCount(0);
    await page.getByRole("button", { name: /Show Archived/i }).click();
    const archivedCard = page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /Experience ADR #14/i })
      .first();
    await expect(archivedCard).toBeVisible();
    await expect(archivedCard).toContainText(/archived/i);
    const log = await getIpcLog(page);
    expect(log.some((entry) => entry.cmd === "archive_experience")).toBeTruthy();
    expect(
      log.some(
        (entry) =>
          entry.cmd === "list_experiences" &&
          Boolean((entry.args as { includeArchived?: boolean } | null)?.includeArchived),
      ),
    ).toBeTruthy();
  });

  test("EX-04 · Pin", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "EchoMind");
    const target = page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /Experience ADR #14/i })
      .first();
    await target.click();
    await page.locator("[data-qa=experience-drawer]").getByRole("button", { name: /^Pin$/i }).click();
    const log = await getIpcLog(page);
    expect(log.some((entry) => entry.cmd === "pin_experience")).toBeTruthy();
    await page.locator("[data-qa=experience-drawer]").getByRole("button", { name: /✕|Close/i }).click();
    const cards = page.locator('[data-qa="experience-card"]');
    await expect(cards.first()).toContainText("PIN");
    await expect(
      page.locator('[data-qa="experience-card"]').filter({ hasText: /Experience ADR #14/i }).first(),
    ).toContainText("PIN");
  });

  test("EX-05 · Agent experiences auto-approved with reviewed=false + unreviewed badge", async ({
    page,
  }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    const badge = page.locator('[data-qa="unreviewed-count"]');
    await expect(badge).toBeVisible();
    const before = Number(((await badge.first().textContent()) || "").trim());
    expect(before).toBeGreaterThan(0);
    const unreviewedCard = page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /unreviewed/i })
      .first();
    await unreviewedCard.click();
    const drawer = page.locator("[data-qa=experience-drawer]");
    await expect(drawer).toBeVisible();
    await expect(drawer.getByRole("button", { name: /Mark reviewed/i })).toHaveCount(0);
    await drawer.getByRole("button", { name: /✕|Close/i }).click();
    await expect(badge).toHaveCount(0);
  });

  test("EX-05b · Mark all reviewed clears badge from backend count", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    // Seed multiple unreviewed active rows, then refetch via Show Archived toggle.
    await page.evaluate(() => {
      const fixture = (
        window as Window & { __QA_FIXTURE__?: { experiences: Array<Record<string, unknown>> } }
      ).__QA_FIXTURE__;
      if (!fixture) return;
      for (const row of fixture.experiences) {
        if ((row.status ?? "active") === "active") {
          row.reviewed = false;
        }
      }
    });
    await page.getByRole("button", { name: /Show Archived/i }).click();
    await page.getByRole("button", { name: /Hide Archived/i }).click();
    const badge = page.locator('[data-qa="unreviewed-count"]');
    await expect(badge.first()).toBeVisible();
    expect(Number(((await badge.first().textContent()) || "").trim())).toBeGreaterThan(1);
    await page.getByRole("button", { name: /Mark all reviewed/i }).click();
    await expect(badge).toHaveCount(0);
    const log = await getIpcLog(page);
    expect(log.some((entry) => entry.cmd === "mark_all_experiences_reviewed")).toBeTruthy();
    expect(log.some((entry) => entry.cmd === "count_unreviewed_experiences")).toBeTruthy();
  });

  test("EX-08 · List limit > 12", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    const cards = page.locator('[data-qa="experience-card"]');
    expect(await cards.count()).toBeGreaterThan(12);
  });

  test("EX-13 · TTL auto-archive restore UI", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    await expect(page.getByRole("button", { name: /Show Archived/i })).toBeVisible();
    await page.getByRole("button", { name: /Show Archived/i }).click();
    const archived = page
      .locator('[data-qa="experience-card"]')
      .filter({ hasText: /Experience ADR #16/i })
      .first();
    await expect(archived).toBeVisible();
    await archived.click();
    const drawer = page.locator("[data-qa=experience-drawer]");
    await expect(drawer).toBeVisible();
    await expect(drawer).toContainText(/Restore from auto-archive/i);
    await drawer.getByRole("button", { name: /^Restore$/i }).click();
    const log = await getIpcLog(page);
    expect(log.some((entry) => entry.cmd === "unarchive_experience")).toBeTruthy();
  });

  test("EX-LAYOUT · Semantic Map + Experiences fill ≥85%", async ({ page }, testInfo) => {
    if (testInfo.project.name === "D4-scale") {
      await openRoute(page, "/settings", "full");
      const scale130 = page
        .getByRole("radio", { name: /130/i })
        .or(page.getByRole("button", { name: /130%/ }));
      if (await scale130.count()) {
        await scale130.first().click();
      } else if (await page.getByText("130%").count()) {
        await page.getByText("130%").first().click();
      }
    }
    await openRoute(page, "/vault", "full");
    const mTop = await measureLayout(page, "/vault");
    expect(mTop.l1_pass && mTop.l3_pass, formatLayoutFailure(mTop)).toBe(true);

    // After project drill-down, page list + experience cards must fill the panels.
    await openVaultProject(page, "Agent-Lounge-OS");
    const m = await measureLayout(page, "/vault");
    const fail = formatLayoutFailure(m);
    testInfo.annotations.push({ type: "layout", description: fail });
    expect(m.l1_pass && m.l3_pass && !m.l2_sparseInterior, fail).toBe(true);

    // Drawer + footer must remain fully visible at 960px / D4-scale (130%) as well as D0–D3.
    await page.locator('[data-qa="experience-card"]').first().click();
    const drawer = page.locator("[data-qa=experience-drawer]");
    await expect(drawer).toBeVisible();
    const footer = drawer.locator('[data-qa="experience-drawer-footer"]');
    await expect(footer).toBeVisible();
    const footerBox = await footer.boundingBox();
    const viewport = page.viewportSize();
    expect(footerBox).toBeTruthy();
    expect(viewport).toBeTruthy();
    expect(footerBox!.y + footerBox!.height).toBeLessThanOrEqual((viewport!.height ?? 0) + 1);
    const header = page.locator('[data-qa="vault-experience-header"]');
    const headerBox = await header.boundingBox();
    expect(headerBox).toBeTruthy();
    expect(headerBox!.width).toBeLessThanOrEqual((viewport!.width ?? 0) + 1);
  });

  test("EX-14 · Vault totals must match bridge counts (not query LIMIT as total)", async ({
    page,
  }) => {
    await openRoute(page, "/vault", "full");
    // Project list shows reconciled totals (not LIMIT-shaped node dumps).
    const projectText = await page.locator('[data-qa="vault-semantic-map"]').innerText();
    const treeNodes = Number(
      (projectText.match(/(\d+)\s*AST nodes/i) || projectText.match(/(\d+)\s*nodes/i) || [])[1] ||
        0,
    );
    const treeEdges = Number((projectText.match(/(\d+)\s*edges/i) || [])[1] || 0);
    await openVaultProject(page, "Agent-Lounge-OS");
    const text = await page.locator("main").innerText();
    const bridge = text.match(/nodes=(\d+)\s+edges=(\d+)/i);
    expect(bridge, "bridge stats appear in experience content").toBeTruthy();
    const bridgeNodes = Number(bridge![1]);
    const bridgeEdges = Number(bridge![2]);
    expect(treeNodes, `tree nodes ${treeNodes} vs bridge ${bridgeNodes}`).toBe(bridgeNodes);
    expect(treeEdges, `tree edges ${treeEdges} vs bridge ${bridgeEdges}`).toBe(bridgeEdges);
  });

  test("EX-15 · Experience Log hides raw markdown dumps and internal TR errors", async ({
    page,
  }) => {
    await openRoute(page, "/vault", "full");
    await openVaultProject(page, "Agent-Lounge-OS");
    const text = await page.locator("main").innerText();
    expect(text).not.toMatch(/##\s*Cross-Project Memory/i);
    expect(text).not.toMatch(/###\s*Tecrübeler/i);
    expect(text).not.toMatch(/memory_bridge hata:/i);
    expect(text).not.toMatch(/repo_path çözümlenemedi/i);
  });

  test("CP-02 · Palette experience opens detail drawer", async ({ page }) => {
    await openRoute(page, "/vault", "full");
    await page.keyboard.press("Control+k");
    await page.getByRole("dialog").locator("input").fill("Experience ADR #4");
    await page.getByRole("dialog").getByText(/Experience ADR #4/i).first().click();
    await expect(page.locator("[data-qa=experience-drawer]")).toBeVisible();
    await expect(page.locator("[data-qa=experience-drawer]")).toContainText("Experience ADR #4");
  });
});

test.describe("DS — dead symbols", () => {
  test("DS-01 · Full list beyond 8", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    const rows = page.locator('[data-qa="dead-symbol-row"]');
    await expect(rows.first()).toBeVisible();
    expect(await rows.count()).toBeGreaterThan(8);
    const total = page.locator('[data-qa="dead-symbol-total"]');
    await expect(total).toBeVisible();
    const listed = Number(await total.getAttribute("data-qa-total"));
    expect(listed).toBeGreaterThan(8);
  });

  test("DS-02 · Detail on click", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    const orphan = page.locator('[data-qa="dead-symbol-row"]').filter({ hasText: "orphan_dispatch" });
    await expect(orphan.first()).toBeVisible();
    await orphan.first().click();
    const detail = page.locator('[data-qa="dead-symbol-detail"]');
    await expect(detail).toBeVisible();
    await expect(detail.getByText("orphan_dispatch")).toBeVisible();
    await expect(detail.getByText(/last_ref/i)).toBeVisible();
    // Fixture last_ref path for orphan_dispatch (EN/TR label already asserted above).
    await expect(detail.getByText(/workflow_engine\.rs:168/i)).toBeVisible();
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

  test("DS-04 · Copy path", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await openRoute(page, "/health?tab=dead", "full");
    await page.locator('[data-qa="dead-symbol-row"]').first().click();
    await page.getByRole("button", { name: /Copy path/i }).click();
    await expect(page.getByText(/Copy path:/i)).toBeVisible();
    const clip = await page.evaluate(() => navigator.clipboard.readText());
    expect(clip).toMatch(/:\d+$/);
    expect(clip.length).toBeGreaterThan(3);
  });

  test("DS-05 · Ignore", async ({ page }) => {
    await openRoute(page, "/health", "full");
    const drill = page.locator(
      'a[data-qa="health-dead-drilldown"][href*="Agent-Lounge-OS"]',
    );
    const headlineBefore = Number(await drill.getAttribute("data-qa-dead-count"));
    expect(headlineBefore).toBeGreaterThan(0);
    await openRoute(page, "/health?tab=dead", "full");
    const totalEl = page.locator('[data-qa="dead-symbol-total"]');
    const beforeTotal = Number(await totalEl.getAttribute("data-qa-total"));
    expect(beforeTotal).toBeGreaterThan(0);
    const rows = page.locator('[data-qa="dead-symbol-row"]');
    const before = await rows.count();
    await rows.first().click();
    const name = (await rows.first().innerText()).split("\n")[0]?.trim() || "";
    await page.getByRole("button", { name: /^Ignore$/i }).click();
    await page.waitForTimeout(400);
    const after = await rows.count();
    expect(after).toBeLessThan(before);
    const afterTotal = Number(await totalEl.getAttribute("data-qa-total"));
    expect(afterTotal).toBe(beforeTotal - 1);
    expect(afterTotal).toBeLessThan(beforeTotal);

    // Headline/KPI on Project Health must decrement with the list.
    await page
      .getByRole("navigation", { name: "Health sections" })
      .getByRole("link", { name: /Project Health/i })
      .click();
    await page.waitForTimeout(300);
    const headlineAfter = Number(
      await page
        .locator('a[data-qa="health-dead-drilldown"][href*="Agent-Lounge-OS"]')
        .getAttribute("data-qa-dead-count"),
    );
    expect(headlineAfter).toBe(headlineBefore - 1);

    // Persist across reload (sessionStorage-backed mock ignore list).
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.waitForTimeout(500);
    await openRoute(page, "/health?tab=dead", "full");
    const reloadedTotal = Number(
      await page.locator('[data-qa="dead-symbol-total"]').getAttribute("data-qa-total"),
    );
    expect(reloadedTotal).toBe(afterTotal);
    if (name) {
      const stillThere = await page
        .locator('[data-qa="dead-symbol-row"]')
        .filter({ hasText: name })
        .count();
      expect(stillThere).toBe(0);
    }
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

  test("DS-LAYOUT · Dead list + detail fill width/height", async ({ page }) => {
    await openRoute(page, "/health?tab=dead", "full");
    const m = await measureLayout(page, "/health");
    expect(m.l1_pass, formatLayoutFailure(m)).toBe(true);
    expect(m.l2_pass, formatLayoutFailure(m)).toBe(true);
    expect(m.l3_pass, formatLayoutFailure(m)).toBe(true);
  });
});
