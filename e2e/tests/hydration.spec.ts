import { expect, test } from "@playwright/test";
import {
  ALL_APP_ROUTES,
  attachHydrationConsoleCollector,
  HYDRATION_PROFILES,
  openRouteHydration,
  withHydrationProfile,
} from "../helpers/hydration";

test.describe("HY — hydration regression", () => {
  for (const profile of HYDRATION_PROFILES) {
    test(`HY-01 · no hydration console noise · ${profile.id}`, async ({ browser }) => {
      await withHydrationProfile(browser, profile, async (page) => {
        const hydrationMessages: { route: string; entries: { type: string; text: string }[] }[] = [];

        const entries = attachHydrationConsoleCollector(page);
        for (const route of ALL_APP_ROUTES) {
          const before = entries.length;
          await openRouteHydration(page, route, route === "/onboarding" ? "empty" : "full");
          if (entries.length > before) {
            hydrationMessages.push({ route, entries: entries.slice(before) });
          }
        }

        const summary = hydrationMessages
          .map(
            (row) =>
              `${row.route}:\n${row.entries.map((entry) => `  [${entry.type}] ${entry.text}`).join("\n")}`,
          )
          .join("\n\n");

        expect(hydrationMessages, summary || "no hydration messages").toEqual([]);
      });
    });
  }

  test("HY-02 · palette shortcut label adapts after mount without hydration errors", async ({
    browser,
  }) => {
    const macProfile = HYDRATION_PROFILES.find((row) => row.id === "macos-en");
    const winProfile = HYDRATION_PROFILES.find((row) => row.id === "windows-en");
    if (!macProfile || !winProfile) {
      throw new Error("hydration profiles missing");
    }

    for (const [profile, pattern] of [
      [macProfile, /⌘\s*K|Cmd\+K/i] as const,
      [winProfile, /Ctrl\+K|Control\+K/i] as const,
    ]) {
      await withHydrationProfile(browser, profile, async (page) => {
        const entries = attachHydrationConsoleCollector(page);
        await openRouteHydration(page, "/dashboard", "full");
        const kbd = page.locator("header kbd").filter({ hasText: /K/ });
        await expect(kbd.first()).toBeVisible();
        await expect(kbd.first()).toHaveText(pattern);
        expect(entries, entries.map((entry) => entry.text).join("\n")).toEqual([]);
      });
    }
  });
});
