import type { Browser, Page } from "@playwright/test";
import { isHydrationConsoleMessage } from "../../src/lib/hydration-console";
import { installTauriMock, waitForAppReady, type FixtureName } from "../harness/tauri-mock";
import { ROUTES } from "./nav";

export { isHydrationConsoleMessage };

export type HydrationProfile = {
  id: string;
  userAgent: string;
  locale: string;
};

/** macOS Safari-like and Windows/Linux Chrome user agents for platform label checks. */
export const HYDRATION_PROFILES: HydrationProfile[] = [
  {
    id: "macos-en",
    userAgent:
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
    locale: "en-US",
  },
  {
    id: "macos-tr",
    userAgent:
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
    locale: "tr-TR",
  },
  {
    id: "windows-en",
    userAgent:
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    locale: "en-US",
  },
  {
    id: "windows-tr",
    userAgent:
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    locale: "tr-TR",
  },
  {
    id: "linux-en",
    userAgent:
      "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    locale: "en-US",
  },
  {
    id: "linux-tr",
    userAgent:
      "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    locale: "tr-TR",
  },
];

export type HydrationConsoleEntry = {
  type: string;
  text: string;
};

export function attachHydrationConsoleCollector(page: Page): HydrationConsoleEntry[] {
  const entries: HydrationConsoleEntry[] = [];
  page.on("console", (msg) => {
    const text = msg.text();
    if (isHydrationConsoleMessage(text)) {
      entries.push({ type: msg.type(), text });
    }
  });
  page.on("pageerror", (err) => {
    const text = String(err);
    if (isHydrationConsoleMessage(text)) {
      entries.push({ type: "pageerror", text });
    }
  });
  return entries;
}

export async function openRouteHydration(
  page: Page,
  pathName: string,
  fixture: FixtureName = "full",
) {
  await installTauriMock(page, fixture);
  const base = process.env.PLAYWRIGHT_BASE_URL || "http://127.0.0.1:4173";
  const res = await page.goto(`${base}${pathName}`, {
    waitUntil: "domcontentloaded",
    timeout: 60_000,
  });
  if (res && res.status() >= 400) {
    throw new Error(`goto ${pathName} status ${res.status()}`);
  }
  await waitForAppReady(page);
  const ready = page.locator("main, [data-qa='panel'], body");
  await ready.first().waitFor({ state: "attached", timeout: 15_000 });
  await page.waitForTimeout(400);
}

export async function withHydrationProfile(
  browser: Browser,
  profile: HydrationProfile,
  run: (page: Page) => Promise<void>,
) {
  const context = await browser.newContext({
    userAgent: profile.userAgent,
    locale: profile.locale,
  });
  const page = await context.newPage();
  await page.addInitScript(() => {
    try {
      localStorage.removeItem("lounge.locale");
      localStorage.removeItem("locale");
      localStorage.removeItem("al-os-ui-scale");
      localStorage.removeItem("al-os-panel-collapsed");
    } catch {
      /* ignore */
    }
  });
  try {
    await run(page);
  } finally {
    await context.close();
  }
}

export const ALL_APP_ROUTES = ROUTES.map((route) => route.path);
