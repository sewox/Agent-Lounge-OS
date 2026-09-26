import type { Page } from "@playwright/test";
import { installTauriMock, waitForAppReady, type FixtureName } from "../harness/tauri-mock";

export const ROUTES = [
  { id: "dashboard", path: "/dashboard" },
  { id: "stream", path: "/stream" },
  { id: "vault", path: "/vault" },
  { id: "health", path: "/health" },
  { id: "fleet", path: "/fleet" },
  { id: "telemetry", path: "/telemetry" },
  { id: "quotas", path: "/quotas" },
  { id: "settings", path: "/settings" },
  { id: "onboarding", path: "/onboarding" },
] as const;

export type RouteId = (typeof ROUTES)[number]["id"];

export async function openRoute(
  page: Page,
  pathName: string,
  fixture: FixtureName = "full",
  opts?: { waitMs?: number },
) {
  await installTauriMock(page, fixture);
  const base = process.env.PLAYWRIGHT_BASE_URL || "http://127.0.0.1:4173";
  const res = await page.goto(`${base}${pathName}`, { waitUntil: "domcontentloaded", timeout: 60_000 });
  if (res && res.status() >= 400) {
    throw new Error(`goto ${pathName} status ${res.status()}`);
  }
  await waitForAppReady(page);
  // Prefer <main>; fall back to onboarding / panel root (SSR always has main, but
  // some client error boundaries can briefly remount).
  const ready = page.locator("main, [data-qa='panel'], body");
  await ready.first().waitFor({ state: "attached", timeout: 15_000 });
  if (opts?.waitMs) {
    await page.waitForTimeout(opts.waitMs);
  }
}

export async function collectConsoleErrors(page: Page) {
  const errors: string[] = [];
  page.on("pageerror", (err) => {
    errors.push(String(err));
  });
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      const text = msg.text();
      // Ignore benign Next/hydration noise in static harness if any.
      if (text.includes("Download the React DevTools")) return;
      errors.push(text);
    }
  });
  return errors;
}
