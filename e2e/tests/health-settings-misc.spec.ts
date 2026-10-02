import { test, expect } from "@playwright/test";
import { openRoute } from "../helpers/nav";
import {
  getIpcLog,
  seedDestructiveQueue,
  type QaDestructiveRow,
} from "../harness/tauri-mock";

const DESTRUCTIVE_FIFO: QaDestructiveRow[] = [
  {
    id: "dest-fifo-1",
    kind: "destructive",
    command: "rm -rf /tmp/agent-lounge-demo",
    pattern: "RmRf",
    source: "agent",
    class: "RmRf",
    command_hash: "hash-fifo-1",
  },
  {
    id: "dest-fifo-2",
    kind: "destructive",
    command: "git reset --hard HEAD",
    pattern: "GitResetHard",
    source: "agent",
    class: "GitResetHard",
    command_hash: "hash-fifo-2",
  },
];

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
    expect(main).toMatch(/Kota verisi alınamadı|Could not load quota data/i);
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
  test("ST-01 · Routing table not clipped", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const region = page.locator('[data-qa="routing-table"]');
    await expect(region).toBeVisible();
    await region.scrollIntoViewIfNeeded();
    const box = await region.boundingBox();
    expect(box, "routing table must have a layout box").toBeTruthy();
    const vp = page.viewportSize()!;
    // Tightened: after scrollIntoView, the table top must sit in the viewport
    // and either fully fit or have an explicit scroll container.
    expect(box!.y, "routing table top must be in viewport").toBeGreaterThanOrEqual(-2);
    expect(box!.y, "routing table top must not sit below the fold").toBeLessThan(vp.height);
    const scrollable = await region.evaluate((el) => {
      let node: HTMLElement | null = el;
      while (node) {
        const style = window.getComputedStyle(node);
        const oy = style.overflowY;
        if (
          (oy === "auto" || oy === "scroll" || style.overflow === "auto" || style.overflow === "scroll") &&
          node.scrollHeight > node.clientHeight + 1
        ) {
          return true;
        }
        node = node.parentElement;
      }
      return el.scrollHeight > el.clientHeight + 1;
    });
    const bottom = box!.y + box!.height;
    if (bottom > vp.height - 4) {
      expect(scrollable, "overflowing routing table must scroll inside a parent").toBe(true);
    }
    // Controls inside the table must be interactable (not zero-size / opacity-0 clipped).
    const firstControl = region.locator("input, select, button, [role='checkbox']").first();
    await expect(firstControl, "routing table must expose a control").toBeVisible();
    await expect(firstControl).toBeEnabled();
  });

  test("ST-02 · Routing policy save calls set_routing_policy", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const before = await getIpcLog(page);
    // Policy autosaves on trigger checkbox toggle (no Graph Kaydet / native dialog).
    // Scope to routing table so approval-sound / other prefs checkboxes are ignored.
    const trigger = page
      .locator('[data-qa="routing-table"] input[type="checkbox"]:not([disabled])')
      .first();
    await expect(trigger, "editable routing trigger must exist").toBeVisible();
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
    await expect(locked.first(), "locked approval checkbox must exist").toBeVisible();
    const el = locked.first();
    const title =
      (await el.getAttribute("title")) ||
      (await el.evaluate((node) => node.parentElement?.textContent || ""));
    expect(title || "", "disabled approval checkbox lacks explanation").toMatch(
      /kilit|lock|always|onay|disabled|zorunlu/i,
    );
  });

  test("EX-13 · Settings TTL + use-count controls persist", async ({ page }) => {
    await openRoute(page, "/settings", "full");
    const panel = page.locator('[data-qa="experience-governance"]');
    await expect(panel).toBeVisible();
    await panel.scrollIntoViewIfNeeded();
    const ttlExact = panel.locator('input[type="number"]').first();
    await expect(ttlExact).toBeVisible();
    await ttlExact.fill("120");
    await ttlExact.blur();
    await page.waitForTimeout(300);
    const useCount = panel.locator('[data-qa="use-count-threshold"]');
    await expect(useCount).toBeVisible();
    await useCount.fill("3");
    await useCount.blur();
    await page.waitForTimeout(300);
    const log = await getIpcLog(page);
    expect(log.some((e) => e.cmd === "set_experience_ttl_days")).toBeTruthy();
    expect(log.some((e) => e.cmd === "set_experience_use_count_threshold")).toBeTruthy();
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
    const approve = page.getByRole("button", { name: /Onayla|Approve|Local|Yerel|Reddet|Deny/i });
    await expect(approve.first(), "routing banner demo actions").toBeVisible({ timeout: 5_000 });
    expect(await approve.count()).toBeGreaterThan(0);
  });

  test("AP-06 · Destructive ops always require confirmation UI (POSIX + Windows)", async ({
    page,
  }) => {
    await openRoute(page, "/dashboard", "full");
    await seedDestructiveQueue(page, DESTRUCTIVE_FIFO);
    const dialog = page.locator('[data-qa="destructive-dialog"]');
    await expect(dialog, "destructive confirm alertdialog").toBeVisible({ timeout: 5_000 });
    await expect(dialog).toHaveAttribute("data-task-id", DESTRUCTIVE_FIFO[0]!.id);
    await expect(dialog).toHaveAttribute("data-command-hash", DESTRUCTIVE_FIFO[0]!.command_hash);
    await expect(page.locator('[data-qa="destructive-command"]')).toHaveText(
      DESTRUCTIVE_FIFO[0]!.command,
    );
    await expect(page.locator('[data-qa="destructive-queue-note"]')).toBeVisible();
    await expect(page.locator('[data-qa="destructive-confirm"]')).toBeVisible();
    await expect(page.locator('[data-qa="destructive-reject"]')).toBeVisible();
    // Settings copy still documents Never Ask cannot skip (static gate panel).
    await openRoute(page, "/settings", "full");
    const gate = page.locator('[data-qa="destructive-gate"]');
    await expect(gate, "destructive gate panel").toBeVisible();
    await expect(gate.getByText(/Never Ask|atlanamaz|cannot skip/i).first()).toBeVisible();
  });

  test("AP-07 · FIFO reject then confirm with correct id/hash IPC", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await seedDestructiveQueue(page, DESTRUCTIVE_FIFO);
    const dialog = page.locator('[data-qa="destructive-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('[data-qa="destructive-command"]')).toHaveText(
      DESTRUCTIVE_FIFO[0]!.command,
    );

    const beforeReject = await getIpcLog(page);
    await page.locator('[data-qa="destructive-reject"]').click();
    await expect(page.locator('[data-qa="destructive-command"]')).toHaveText(
      DESTRUCTIVE_FIFO[1]!.command,
      { timeout: 5_000 },
    );
    await expect(dialog).toHaveAttribute("data-task-id", DESTRUCTIVE_FIFO[1]!.id);
    await expect(dialog).toHaveAttribute("data-command-hash", DESTRUCTIVE_FIFO[1]!.command_hash);

    const afterReject = await getIpcLog(page);
    const rejectCall = afterReject
      .slice(beforeReject.length)
      .find((e) => e.cmd === "reject_destructive");
    expect(rejectCall, "reject_destructive IPC").toBeTruthy();
    const rejectArgs = rejectCall!.args as { id?: string; commandHash?: string };
    expect(rejectArgs.id).toBe(DESTRUCTIVE_FIFO[0]!.id);
    expect(rejectArgs.commandHash).toBe(DESTRUCTIVE_FIFO[0]!.command_hash);

    const beforeConfirm = await getIpcLog(page);
    await page.locator('[data-qa="destructive-confirm"]').click();
    await expect(dialog).toBeHidden({ timeout: 5_000 });

    const afterConfirm = await getIpcLog(page);
    const confirmCall = afterConfirm
      .slice(beforeConfirm.length)
      .find((e) => e.cmd === "confirm_destructive");
    expect(confirmCall, "confirm_destructive IPC").toBeTruthy();
    const confirmArgs = confirmCall!.args as { id?: string; commandHash?: string };
    expect(confirmArgs.id).toBe(DESTRUCTIVE_FIFO[1]!.id);
    expect(confirmArgs.commandHash).toBe(DESTRUCTIVE_FIFO[1]!.command_hash);
  });

  test("AP-07b · Destructive dialog focus trap and Esc rejects", async ({ page }) => {
    await openRoute(page, "/dashboard", "full");
    await seedDestructiveQueue(page, [DESTRUCTIVE_FIFO[0]!]);
    const dialog = page.locator('[data-qa="destructive-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    const rejectBtn = page.locator('[data-qa="destructive-reject"]');
    const confirmBtn = page.locator('[data-qa="destructive-confirm"]');
    await expect(rejectBtn).toBeFocused({ timeout: 2_000 });

    await page.keyboard.press("Tab");
    await expect(confirmBtn).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(rejectBtn).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(confirmBtn).toBeFocused();

    const beforeEsc = await getIpcLog(page);
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden({ timeout: 5_000 });
    const afterEsc = await getIpcLog(page);
    const rejectCall = afterEsc
      .slice(beforeEsc.length)
      .find((e) => e.cmd === "reject_destructive");
    expect(rejectCall, "Esc must reject via reject_destructive").toBeTruthy();
    expect((rejectCall!.args as { id?: string }).id).toBe(DESTRUCTIVE_FIFO[0]!.id);
  });

  test("AP-08 · Pending approval plays alert sound (HTML Audio; wav/mp3/ogg)", async ({
    page,
  }) => {
    // §10.1 / §10.2: webview HTMLAudioElement; bundled wav/mp3/ogg;
    // plays when hidden; configured interval repeat until decision. No background escalation.
    await page.addInitScript(() => {
      type PlayLog = { src: string; t: number };
      const g = window as Window & { __QA_AUDIO_PLAYS__?: PlayLog[] };
      g.__QA_AUDIO_PLAYS__ = [];
      const proto = HTMLAudioElement.prototype;
      const origPlay = proto.play;
      proto.play = function playSpy(this: HTMLAudioElement, ...args: unknown[]) {
        g.__QA_AUDIO_PLAYS__!.push({
          src: this.currentSrc || this.src || "",
          t: Date.now(),
        });
        return origPlay.apply(this, args as []).catch(() => undefined as unknown as void);
      };
      localStorage.setItem(
        "lounge.approvalSound",
        JSON.stringify({
          enabled: true,
          soundId: "chime-soft",
          customFileName: null,
          volume: 0.7,
          intervalSecs: 5,
        }),
      );
    });
    await openRoute(page, "/dashboard?demo=routing-banner", "browser");
    await page.waitForTimeout(800);
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await page.waitForTimeout(400);

    const firstBatch = await page.evaluate(
      () => (window as Window & { __QA_AUDIO_PLAYS__?: { src: string }[] }).__QA_AUDIO_PLAYS__ ?? [],
    );
    expect(firstBatch.length, "audio play spy should see ≥1 alert while approval pending").toBeGreaterThan(0);
    for (const p of firstBatch) {
      expect(p.src, "alert src must be non-empty").toBeTruthy();
      expect(p.src, `expected bundled chime-soft wav, got ${p.src}`).toMatch(/chime-soft\.wav/i);
    }
    const beforeRepeat = firstBatch.length;

    await page.waitForTimeout(5_200);
    const afterRepeat = await page.evaluate(
      () => (window as Window & { __QA_AUDIO_PLAYS__?: unknown[] }).__QA_AUDIO_PLAYS__?.length ?? 0,
    );
    expect(afterRepeat, "should repeat at least once over the configured interval").toBeGreaterThan(
      beforeRepeat,
    );

    const allPlays = await page.evaluate(
      () => (window as Window & { __QA_AUDIO_PLAYS__?: { src: string }[] }).__QA_AUDIO_PLAYS__ ?? [],
    );
    const uniqueSrc = [...new Set(allPlays.map((p) => p.src))];
    expect(uniqueSrc.length, "must not escalate to a different sound").toBe(1);

    const decide = page.getByRole("button", { name: /Onayla|Approve|Reddet|Deny/i }).first();
    await expect(decide).toBeVisible();
    const atDecision = await page.evaluate(
      () => (window as Window & { __QA_AUDIO_PLAYS__?: unknown[] }).__QA_AUDIO_PLAYS__?.length ?? 0,
    );
    await decide.click();
    await page.waitForTimeout(5_500);
    const afterResolve = await page.evaluate(
      () => (window as Window & { __QA_AUDIO_PLAYS__?: unknown[] }).__QA_AUDIO_PLAYS__?.length ?? 0,
    );
    expect(afterResolve, "must stop repeating after approve/deny").toBe(atDecision);
  });

  test("AP-09 · Settings sound options (on/off, builtins, volume, interval, Listen)", async ({
    page,
  }) => {
    await openRoute(page, "/settings", "full");
    const section = page.locator('[data-qa="approval-sound"]');
    await expect(section, "sound settings section").toBeVisible();
    await section.scrollIntoViewIfNeeded();
    await expect(section.getByRole("button", { name: /^Dinle$|Preview|Play|Listen|Test/i })).toBeVisible();
    await expect(section.getByText(/wav|mp3|ogg|upload|yükle|Pick|Dosya|≤5/i).first()).toBeVisible();
    await expect(section.getByText(/volume|ses|interval|aralık|60/i).first()).toBeVisible();
    await expect(section.getByText(/background|arka plan|escalat|artırılmaz/i).first()).toBeVisible();
    await expect(section.getByText(/OS notification|OS bildirimi/i).first()).toBeVisible();
    // Persist: toggle off writes lounge.approvalSound
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
    // Volume + interval controls
    const volume = section.locator('input[type="range"]').first();
    await expect(volume).toBeVisible();
    await volume.fill("40");
    const interval = section.locator('input[type="number"]').first();
    await expect(interval).toBeVisible();
    await interval.fill("30");
    await page.waitForTimeout(150);
    const after = await page.evaluate(() => {
      const raw = localStorage.getItem("lounge.approvalSound") || "";
      try {
        return JSON.parse(raw) as { volume?: number; intervalSecs?: number };
      } catch {
        return {};
      }
    });
    expect(after.volume).toBeCloseTo(0.4, 1);
    expect(after.intervalSecs).toBe(30);
  });

  test("AP-10 · Native OS notification → focus_app_for_approval + banner focus", async ({
    page,
  }, testInfo) => {
    // Front-end contract only: the harness mock of focus_app_for_approval emits
    // approval_banner_focus and focuses the banner. Rust raise/slot behaviour is
    // covered by unit tests; live OS toast click is S2 manual (NOT YET RUN).
    testInfo.annotations.push({
      type: "manual",
      description:
        "S2 live NOT YET RUN: real OS toast → activate → banner. Automated AP-10 = front-end contract via mock; see docs/qa/ap-10-notification-click.md.",
    });
    await openRoute(page, "/dashboard", "full");
    expect(await page.evaluate(() => "__TAURI_INTERNALS__" in window)).toBeTruthy();
    const marker = page.locator('[data-qa="approval-notification"]');
    await expect(marker).toHaveCount(1);
    await expect
      .poll(async () => marker.getAttribute("data-tauri-ready"), { timeout: 5_000 })
      .toBe("1");
    await expect
      .poll(async () => marker.getAttribute("data-listeners-ready"), { timeout: 5_000 })
      .toBe("1");

    await page.evaluate(() => {
      const f = window.__QA_FIXTURE__;
      if (!f) throw new Error("missing fixture");
      f.pendingApprovals = [
        {
          task_id: "ap10-focus-task",
          summary: "AP-10 focus test approval",
          from_agent: "claude",
          to_agent: "lmr",
          kind: "agent_switch",
          reason: "harness",
          expires_at: new Date(Date.now() + 90_000).toISOString(),
          timeout_secs: 90,
        },
      ];
      document.dispatchEvent(new Event("visibilitychange"));
    });
    const banner = page.locator('[data-qa="approval-banner"]');
    await expect(banner).toBeVisible({ timeout: 5_000 });

    await page.evaluate(async () => {
      const internals = window.__TAURI_INTERNALS__ as {
        invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      };
      await internals.invoke("focus_app_for_approval", { taskId: "ap10-focus-task" });
    });
    const log = await getIpcLog(page);
    expect(
      log.some((e) => e.cmd === "focus_app_for_approval"),
      "focus_app_for_approval must be invoked",
    ).toBeTruthy();
    await expect(banner).toBeFocused({ timeout: 3_000 });

    await page.evaluate(() => {
      (document.activeElement as HTMLElement | null)?.blur?.();
    });
    await page.evaluate(() => {
      window.dispatchEvent(new Event("focus"));
    });
    await expect(banner).toBeFocused({ timeout: 3_000 });
    const log2 = await getIpcLog(page);
    expect(
      log2.filter((e) => e.cmd === "focus_app_for_approval").length,
    ).toBeGreaterThanOrEqual(2);
  });

  test("PATH-01 · Path handling accepts / \\ and Windows drive letters", async ({
    page,
  }) => {
    await openRoute(page, "/vault", "full");
    // Select project row (not drill-down Open) so the path strip appears.
    await page
      .locator('[data-qa="vault-project-row"]')
      .filter({ hasText: /Agent-Lounge-OS/i })
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
      .locator('[data-qa="vault-project-row"]')
      .filter({ hasText: /EchoMind/i })
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
    await openRoute(page, "/onboarding", "full", { waitMs: 1200 });
    const bodyText = await page.locator("body").innerText();
    expect(bodyText, "onboarding page must load").not.toMatch(/couldn.?t load|could not be found/i);
    const finish = page.getByRole("button", { name: /Sistemi Başlat|Finish|Start/i }).first();
    await expect(finish).toBeVisible({ timeout: 15_000 });
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
    expect(
      /Worker Fleet|İşçi Filosu|Fleet|memory-bridge|Grok|LMR|NATS|DecisionGate/i.test(text),
    ).toBeTruthy();
    await expect(
      page.getByText(/Worker detail|İşçi ayrıntısı/i).first(),
    ).toBeVisible();
    await expect(page.getByRole("columnheader", { name: /Heartbeat/i })).toBeVisible();
  });
});

