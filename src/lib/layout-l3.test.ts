/**
 * L3 banner occupancy — negative controls (a–d) + positive/union (e–g).
 * Geometry + DOM collection share `scripts/qa/layout-l3.mjs` with the e2e harness.
 */
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { JSDOM } from "jsdom";
import {
  STATUS_BANNER_STACK_SELECTOR,
  L3_EMPTY_RATIO_MAX,
  L3_GRID,
  clipRectToViewport,
  collectStatusBannerRects,
  l3ContentRect,
  largestEmptyRatio,
  l3Pass,
} from "../../scripts/qa/layout-l3.mjs";

type Rect = { x: number; y: number; w: number; h: number };

const VP = { width: 800, height: 800 };

/**
 * jsdom does not layout; elements opt into geometry via `data-rect="x,y,w,h"`.
 */
function withDom(html: string, run: () => void) {
  const dom = new JSDOM(html, {
    pretendToBeVisual: true,
    url: "http://localhost/",
  });
  const prev = {
    window: globalThis.window,
    document: globalThis.document,
    getComputedStyle: globalThis.getComputedStyle,
    Element: globalThis.Element,
    HTMLElement: globalThis.HTMLElement,
  };
  Object.defineProperty(dom.window, "innerWidth", { value: VP.width, configurable: true });
  Object.defineProperty(dom.window, "innerHeight", { value: VP.height, configurable: true });

  const proto = dom.window.HTMLElement.prototype;
  const originalGbr = proto.getBoundingClientRect;
  proto.getBoundingClientRect = function getBoundingClientRect() {
    const raw = this.getAttribute("data-rect");
    if (raw) {
      const [x, y, w, h] = raw.split(",").map(Number);
      return {
        x: x!,
        y: y!,
        width: w!,
        height: h!,
        top: y!,
        left: x!,
        right: x! + w!,
        bottom: y! + h!,
        toJSON() {
          return this;
        },
      } as DOMRect;
    }
    return originalGbr.call(this);
  };

  // @ts-expect-error test shim
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;
  globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
  // @ts-expect-error test shim
  globalThis.Element = dom.window.Element;
  // @ts-expect-error test shim
  globalThis.HTMLElement = dom.window.HTMLElement;
  try {
    run();
  } finally {
    proto.getBoundingClientRect = originalGbr;
    // @ts-expect-error restore
    globalThis.window = prev.window;
    globalThis.document = prev.document;
    globalThis.getComputedStyle = prev.getComputedStyle;
    // @ts-expect-error restore
    globalThis.Element = prev.Element;
    // @ts-expect-error restore
    globalThis.HTMLElement = prev.HTMLElement;
    dom.window.close();
  }
}

describe("layout-l3 shared constants", () => {
  it("keeps the locked banner selector and 15% / 8×8 thresholds", () => {
    assert.equal(STATUS_BANNER_STACK_SELECTOR, '[data-qa="status-banner-stack"]');
    assert.equal(L3_EMPTY_RATIO_MAX, 0.15);
    assert.equal(L3_GRID, 8);
  });
});

describe("L3 negative controls — must still FAIL", () => {
  it("(a) empty band plus a banner — banner does not fill the empty region", () => {
    const content = l3ContentRect({ viewport: VP, sidebarW: 0 });
    const panels: Rect[] = [{ x: 0, y: 500, w: 300, h: 300 }];
    const banners: Rect[] = [{ x: 0, y: 0, w: 800, h: 48 }];
    const ratio = largestEmptyRatio(content, [...panels, ...banners]);
    assert.ok(ratio > L3_EMPTY_RATIO_MAX, `expected L3 fail, got empty=${ratio}`);
    assert.equal(l3Pass(ratio), false);
  });

  it("(b) empty page with only banners", () => {
    const content = l3ContentRect({ viewport: VP, sidebarW: 0 });
    const banners: Rect[] = [
      { x: 0, y: 0, w: 800, h: 40 },
      { x: 0, y: 40, w: 800, h: 40 },
    ];
    const ratio = largestEmptyRatio(content, banners);
    assert.ok(ratio > L3_EMPTY_RATIO_MAX, `expected L3 fail with banners-only, got ${ratio}`);
    assert.equal(l3Pass(ratio), false);
  });

  it("(c) invisible / zero-height / opacity-0 banners count as nothing", () => {
    // Baseline: display:none, visibility:hidden, zero-height childless.
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
          <div data-rect="0,0,800,200" style="display:none">x</div>
          <div data-rect="0,0,800,200" style="visibility:hidden">x</div>
          <div data-rect="0,0,800,0">x</div>
        </div>
      </body></html>`,
      () => {
        assert.equal(collectStatusBannerRects().length, 0);
      },
    );

    // opacity:0 banner with a visible-sized child — must not leak via descendants.
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
          <div data-rect="0,0,800,400" style="opacity:0">
            <div data-rect="0,0,800,400">child</div>
          </div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 0, `opacity:0+child must be empty, got ${JSON.stringify(rects)}`);
        assert.equal(l3Pass(largestEmptyRatio(l3ContentRect({ viewport: VP }), rects)), false);
      },
    );

    // opacity:0 stack with an otherwise visible child — sized so rejection is
    // via opacity, not the zero-size check.
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack" style="opacity:0" data-rect="0,0,800,400">
          <div data-rect="0,0,800,400">child</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 0, `opacity:0 stack must be empty, got ${JSON.stringify(rects)}`);
      },
    );

    // opacity:0 ancestor *above* the stack — jsdom walk must reach documentElement.
    withDom(
      `<!doctype html><html><body>
        <div style="opacity:0" data-rect="0,0,800,400">
          <div data-qa="status-banner-stack" style="display:contents">
            <div data-rect="0,0,800,400">child</div>
          </div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(
          rects.length,
          0,
          `opacity:0 ancestor above stack must be empty, got ${JSON.stringify(rects)}`,
        );
      },
    );

    // zero-height banner with a sized child — no descendant fallback.
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
          <div data-rect="0,0,800,0">
            <div data-rect="0,0,800,400">overflow child</div>
          </div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 0, `zero-height+child must be empty, got ${JSON.stringify(rects)}`);
      },
    );
  });

  it("(d) l3ContentRect ignores <main> padding — harness regression guard", () => {
    withDom(
      `<!doctype html><html><body>
        <aside data-qa="sidebar" data-rect="0,0,200,800"></aside>
        <main data-rect="200,0,600,800" style="padding-top:300px">
          <div data-qa="panel" data-rect="200,300,600,500">panel starts after main padding</div>
        </main>
        <div data-qa="status-banner-stack" style="display:contents">
          <div data-rect="200,48,600,40">banner</div>
        </div>
      </body></html>`,
      () => {
        const main = document.querySelector("main")!;
        const padTop = Number.parseFloat(getComputedStyle(main).paddingTop || "0");
        assert.ok(padTop >= 300, `expected main paddingTop ≥300, got ${padTop}`);

        const content = l3ContentRect({
          viewport: VP,
          sidebarW: 200,
          mainX: 200,
          // Reintroduced exclusion keys must be ignored by the shared helper.
          mainPadTop: padTop,
          mainY: padTop,
        });
        assert.equal(content.y, 0, "contentY must stay 0");
        assert.equal(content.h, VP.height, "contentH must stay viewport height");
        assert.equal(content.x, 200);
        assert.equal(content.w, 600);

        const panels: Rect[] = [{ x: 200, y: 300, w: 600, h: 500 }];
        const banners = collectStatusBannerRects();
        assert.equal(banners.length, 1);

        // Full viewport: empty band in the padded top (banner only covers ~1 row) → fail.
        const ratio = largestEmptyRatio(content, [...panels, ...banners]);
        assert.ok(
          ratio > L3_EMPTY_RATIO_MAX,
          `full-viewport L3 must still fail with padded main, got ${ratio}`,
        );
        assert.equal(l3Pass(ratio), false);

        // Sanity: the old mainPadTop exclusion starts content at the panel → incorrectly passes.
        const regressive: Rect = {
          x: content.x,
          y: padTop,
          w: content.w,
          h: VP.height - padTop,
        };
        const trickRatio = largestEmptyRatio(regressive, [...panels, ...banners]);
        assert.ok(
          trickRatio <= L3_EMPTY_RATIO_MAX,
          "sanity: mainPadTop exclusion would incorrectly pass L3",
        );
      },
    );
  });
});

describe("L3 positive / union controls", () => {
  it("(e) stacked or overlapping banners count once (union)", () => {
    const content = l3ContentRect({ viewport: VP });
    const a: Rect = { x: 0, y: 0, w: 800, h: 400 };
    const b: Rect = { x: 0, y: 0, w: 800, h: 400 };
    const one = largestEmptyRatio(content, [a]);
    const two = largestEmptyRatio(content, [a, b]);
    assert.equal(one, two);
    assert.ok(two > L3_EMPTY_RATIO_MAX);

    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack" style="display:contents">
          <div data-rect="0,0,800,80">A</div>
          <div data-rect="0,20,800,80">B</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 2);
        const unionCover: Rect = { x: 0, y: 0, w: 800, h: 100 };
        const ratio = largestEmptyRatio(content, rects);
        assert.equal(ratio, largestEmptyRatio(content, [unionCover]));
      },
    );
  });

  it("(f) banner partly outside the viewport counts only the visible part", () => {
    const partial = clipRectToViewport({ x: -100, y: -50, w: 300, h: 150 }, VP);
    assert.deepEqual(partial, { x: 0, y: 0, w: 200, h: 100 });

    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack" style="display:contents">
          <div data-rect="0,-40,800,80">partial</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 1);
        assert.deepEqual(rects[0], { x: 0, y: 0, w: 800, h: 40 });
      },
    );
  });

  it("(g) a fixed popup outside the selector does not count", () => {
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack" style="display:contents"></div>
        <div data-rect="0,0,800,800" style="position:fixed">random popup</div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 0);
        const content = l3ContentRect({ viewport: VP });
        assert.equal(largestEmptyRatio(content, rects), 1);
        assert.equal(l3Pass(1), false);
      },
    );
  });

  it("visible banner under the locked selector occupies cells", () => {
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack" style="display:contents">
          <div data-rect="0,0,800,400">banner</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 1);
        assert.deepEqual(rects[0], { x: 0, y: 0, w: 800, h: 400 });
        const content = l3ContentRect({ viewport: VP });
        assert.equal(largestEmptyRatio(content, rects), 0.5);
      },
    );
  });
});
