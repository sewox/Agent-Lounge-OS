/**
 * Real-browser proof that L3 treats `[data-qa="status-banner-stack"]` as occupied
 * (page.evaluate(collectStatusBannerRects) path) and still fails on an empty band.
 * Does not alter assertions in other specs.
 */
import { test, expect } from "@playwright/test";
import {
  collectStatusBannerRects,
  l3ContentRect,
  largestEmptyRatio,
  l3Pass,
  L3_EMPTY_RATIO_MAX,
} from "../../scripts/qa/layout-l3.mjs";
import { measureLayout } from "../helpers/layout";
import { openRoute } from "../helpers/nav";

test.describe("L3 status-banner-stack occupancy (PR-L3-Banner-1)", () => {
  test("L3-BANNER-01 · visible stack occupies cells; empty band below still fails", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "L3-BANNER-01" });
    await page.setViewportSize({ width: 800, height: 800 });
    await page.setContent(
      `<!doctype html>
      <html>
        <body style="margin:0">
          <aside data-qa="sidebar"
            style="position:fixed;left:0;top:0;width:200px;height:100%;background:#222"></aside>
          <div data-qa="status-banner-stack" style="display:contents">
            <div style="position:fixed;top:0;left:200px;right:0;height:80px;background:#c44;z-index:5">
              status banner
            </div>
          </div>
          <main style="margin-left:200px;padding-top:500px;min-height:100vh;box-sizing:border-box">
            <div data-qa="panel"
              style="width:100%;height:280px;background:#444;box-sizing:border-box">
              low panel — empty band above (banner covers only the top strip)
            </div>
          </main>
        </body>
      </html>`,
      { waitUntil: "domcontentloaded" },
    );

    const banners = await page.evaluate(collectStatusBannerRects);
    expect(banners.length, "visible status-banner-stack must yield ≥1 rect").toBeGreaterThan(0);
    expect(banners[0]!.h, "banner height must be positive").toBeGreaterThan(0);

    const content = l3ContentRect({
      viewport: { width: 800, height: 800 },
      sidebarW: 200,
      mainX: 200,
      mainPadTop: 200,
    });
    expect(content.y).toBe(0);
    expect(content.h).toBe(800);

    const panels = await page.evaluate(() => {
      return Array.from(document.querySelectorAll('[data-qa="panel"]')).map((el) => {
        const r = el.getBoundingClientRect();
        return { x: r.x, y: r.y, w: r.width, h: r.height };
      });
    });
    expect(panels.length).toBe(1);

    const withoutBanners = largestEmptyRatio(content, panels);
    const withBanners = largestEmptyRatio(content, [...panels, ...banners]);
    expect(
      withBanners,
      "visible banners must reduce empty ratio vs panels-only",
    ).toBeLessThan(withoutBanners);
    expect(withBanners, "fixture arithmetic: panels∪banners empty ratio").toBe(0.5);
    expect(
      withBanners > L3_EMPTY_RATIO_MAX,
      `empty band below banners must still fail L3 (got ${withBanners})`,
    ).toBe(true);
    expect(l3Pass(withBanners)).toBe(false);

    // Harness path: measureLayout uses the same collector + l3ContentRect.
    // Exact match proves ...bannerRects is in l3Occupied (panels-only is 0.625).
    const metrics = await measureLayout(page, "/l3-banner-fixture");
    expect(metrics.l3_pass, `measureLayout L3 must fail: empty=${metrics.l3_largestEmptyRatio}`).toBe(
      false,
    );
    expect(metrics.l3_largestEmptyRatio).toBeGreaterThan(L3_EMPTY_RATIO_MAX);
    expect(
      metrics.l3_largestEmptyRatio,
      "measureLayout must equal panels∪banners (proves bannerRects are occupied)",
    ).toBe(withBanners);
  });

  test("L3-BANNER-01 · hidden stack (opacity/visibility) yields no banner rects", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "L3-BANNER-01-hidden" });
    await page.setViewportSize({ width: 800, height: 800 });

    for (const hide of [
      { style: "opacity:0", label: "opacity:0" },
      { style: "visibility:hidden", label: "visibility:hidden" },
    ] as const) {
      // Sized block stack (not display:contents) so opacity/visibility apply as
      // ancestors of the fixed banner child — exercises Chromium checkVisibility.
      await page.setContent(
        `<!doctype html>
        <html>
          <body style="margin:0">
            <aside data-qa="sidebar"
              style="position:fixed;left:0;top:0;width:200px;height:100%;background:#222"></aside>
            <div data-qa="status-banner-stack"
              style="position:fixed;top:0;left:200px;right:0;height:80px;${hide.style}">
              <div style="position:absolute;inset:0;background:#c44">
                status banner
              </div>
            </div>
            <main style="margin-left:200px;padding-top:500px;min-height:100vh;box-sizing:border-box">
              <div data-qa="panel"
                style="width:100%;height:280px;background:#444;box-sizing:border-box">
                low panel
              </div>
            </main>
          </body>
        </html>`,
        { waitUntil: "domcontentloaded" },
      );

      const banners = await page.evaluate(collectStatusBannerRects);
      expect(
        banners,
        `Chromium collector must ignore ${hide.label} stack`,
      ).toEqual([]);
    }
  });

  test("L3-BANNER-02 · app shell routing-banner demo occupies via real stack", async ({
    page,
  }, testInfo) => {
    testInfo.annotations.push({ type: "id", description: "L3-BANNER-02" });
    await openRoute(page, "/dashboard?demo=routing-banner", "browser");
    const stack = page.locator('[data-qa="status-banner-stack"]');
    await expect(stack, "status-banner-stack wrapper").toBeAttached();
    const banner = page.locator('[data-qa="approval-banner"]');
    await expect(banner, "routing demo approval banner").toBeVisible({ timeout: 10_000 });

    const metrics = await measureLayout(page, "/dashboard?demo=routing-banner");
    expect(metrics.main, "measureLayout must report main geometry").toEqual(
      expect.objectContaining({ x: expect.any(Number), w: expect.any(Number) }),
    );
    const banners = await page.evaluate(collectStatusBannerRects);
    expect(banners.length, "app shell stack must expose visible banner rects").toBeGreaterThan(0);
    // Same panel filter as measureLayout (drop zero-size boxes).
    const panels = (
      await page.evaluate(() =>
        Array.from(document.querySelectorAll('main [data-qa="panel"]')).map((el) => {
          const r = el.getBoundingClientRect();
          return { x: r.x, y: r.y, w: r.width, h: r.height };
        }),
      )
    ).filter((p) => p.w > 0 && p.h > 0);
    const content = l3ContentRect({
      viewport: metrics.viewport,
      sidebarW: metrics.sidebarW,
      mainX: metrics.main!.x,
    });
    const panelsAndBanners = largestEmptyRatio(content, [...panels, ...banners]);
    expect(
      metrics.l3_largestEmptyRatio,
      "measureLayout must equal panels∪banners with main.x (proves bannerRects + exact content rect)",
    ).toBe(panelsAndBanners);
  });
});
