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
  largestEmptyRatio,
  l3Pass,
} from "../../scripts/qa/layout-l3.mjs";

type Rect = { x: number; y: number; w: number; h: number };

const VP = { width: 800, height: 800 };
/** Full-viewport content area — same contract as measureLayout (contentY=0, contentH=vp.height). */
const FULL_CONTENT: Rect = { x: 0, y: 0, w: VP.width, h: VP.height };

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
    const panels: Rect[] = [{ x: 0, y: 500, w: 300, h: 300 }];
    const banners: Rect[] = [{ x: 0, y: 0, w: 800, h: 48 }];
    const ratio = largestEmptyRatio(FULL_CONTENT, [...panels, ...banners]);
    assert.ok(ratio > L3_EMPTY_RATIO_MAX, `expected L3 fail, got empty=${ratio}`);
    assert.equal(l3Pass(ratio), false);
  });

  it("(b) empty page with only banners", () => {
    const banners: Rect[] = [
      { x: 0, y: 0, w: 800, h: 40 },
      { x: 0, y: 40, w: 800, h: 40 },
    ];
    const ratio = largestEmptyRatio(FULL_CONTENT, banners);
    assert.ok(ratio > L3_EMPTY_RATIO_MAX, `expected L3 fail with banners-only, got ${ratio}`);
    assert.equal(l3Pass(ratio), false);
  });

  it("(c) invisible / zero-height banner counts as nothing", () => {
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
          <div data-rect="0,0,800,200" style="display:none">x</div>
          <div data-rect="0,0,800,200" style="visibility:hidden">x</div>
          <div data-rect="0,0,800,0">x</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 0, `invisible banners must yield no rects, got ${JSON.stringify(rects)}`);
        const ratio = largestEmptyRatio(FULL_CONTENT, rects);
        assert.equal(ratio, 1);
        assert.equal(l3Pass(ratio), false);
      },
    );
  });

  it("(d) excluding <main> padding from content area is the old trick — full viewport still fails", () => {
    const panels: Rect[] = [{ x: 0, y: 200, w: 800, h: 600 }];
    const fullRatio = largestEmptyRatio(FULL_CONTENT, panels);
    assert.ok(
      fullRatio > L3_EMPTY_RATIO_MAX,
      `full-viewport content must fail L3, got ${fullRatio}`,
    );

    const paddedContent: Rect = { x: 0, y: 200, w: 800, h: 600 };
    const trickRatio = largestEmptyRatio(paddedContent, panels);
    assert.ok(
      trickRatio <= L3_EMPTY_RATIO_MAX,
      "sanity: padded content area would hide the defect",
    );
    assert.equal(FULL_CONTENT.y, 0);
    assert.equal(FULL_CONTENT.h, VP.height);
  });
});

describe("L3 positive / union controls", () => {
  it("(e) stacked or overlapping banners count once (union)", () => {
    const a: Rect = { x: 0, y: 0, w: 800, h: 400 };
    const b: Rect = { x: 0, y: 0, w: 800, h: 400 };
    const one = largestEmptyRatio(FULL_CONTENT, [a]);
    const two = largestEmptyRatio(FULL_CONTENT, [a, b]);
    assert.equal(one, two);
    assert.ok(two > L3_EMPTY_RATIO_MAX);

    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
          <div data-rect="0,0,800,80">A</div>
          <div data-rect="0,20,800,80">B</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 2);
        // Union of overlapping bands equals the covering rect (y=0..100), not 2× area.
        const unionCover: Rect = { x: 0, y: 0, w: 800, h: 100 };
        const ratio = largestEmptyRatio(FULL_CONTENT, rects);
        assert.equal(ratio, largestEmptyRatio(FULL_CONTENT, [unionCover]));
      },
    );
  });

  it("(f) banner partly outside the viewport counts only the visible part", () => {
    const partial = clipRectToViewport({ x: -100, y: -50, w: 300, h: 150 }, VP);
    assert.deepEqual(partial, { x: 0, y: 0, w: 200, h: 100 });

    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
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
        <div data-qa="status-banner-stack"></div>
        <div data-rect="0,0,800,800" style="position:fixed">random popup</div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 0);
        const ratio = largestEmptyRatio(FULL_CONTENT, rects);
        assert.equal(ratio, 1);
        assert.equal(l3Pass(ratio), false);
      },
    );
  });

  it("visible banner under the locked selector occupies cells", () => {
    withDom(
      `<!doctype html><html><body>
        <div data-qa="status-banner-stack">
          <div data-rect="0,0,800,400">banner</div>
        </div>
      </body></html>`,
      () => {
        const rects = collectStatusBannerRects();
        assert.equal(rects.length, 1);
        assert.deepEqual(rects[0], { x: 0, y: 0, w: 800, h: 400 });
        const ratio = largestEmptyRatio(FULL_CONTENT, rects);
        assert.equal(ratio, 0.5);
      },
    );
  });
});
