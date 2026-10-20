/**
 * Shared L3 empty-region geometry for Playwright (`e2e/helpers/layout.ts`) and
 * the baseline report (`scripts/qa/generate-baseline-report.mjs`).
 *
 * L3 content area is always the full viewport (`l3ContentRect`: y = 0,
 * h = vp.height). Visible status banners under
 * `[data-qa="status-banner-stack"]` count as occupied cells (same as panels).
 * No generic position:fixed heuristic — only that explicit selector.
 */

/** Locked selector — do not broaden to position:fixed or other chrome. */
export const STATUS_BANNER_STACK_SELECTOR = '[data-qa="status-banner-stack"]';

/** L3 fail when largest empty 8×8 block exceeds this fraction of content area. */
export const L3_EMPTY_RATIO_MAX = 0.15;

export const L3_GRID = 8;

/**
 * @typedef {{ x: number, y: number, w: number, h: number }} Rect
 * @typedef {{ width: number, height: number }} ViewportSize
 */

/**
 * Canonical L3 content area: full viewport height, sidebar-excluded width.
 * Never subtracts header, `<main>` padding, or banner reservation.
 *
 * Extra keys such as `mainPadTop` / `mainY` are ignored on purpose — a
 * reintroduced padding exclusion must not be expressible through this API.
 *
 * @param {{
 *   viewport: ViewportSize,
 *   sidebarW?: number,
 *   mainX?: number,
 *   mainPadTop?: number,
 *   mainY?: number,
 * }} input
 * @returns {Rect}
 */
export function l3ContentRect(input) {
  const viewport = input.viewport;
  const sidebarW = input.sidebarW ?? 0;
  const contentX = input.mainX ?? sidebarW;
  // Intentionally ignore mainPadTop / mainY — L3 content is the full viewport.
  void input.mainPadTop;
  void input.mainY;
  return {
    x: contentX,
    y: 0,
    w: viewport.width - sidebarW,
    h: viewport.height,
  };
}

/**
 * Clip a rect to the viewport; return null if nothing remains visible.
 * @param {Rect} rect
 * @param {ViewportSize} viewport
 * @returns {Rect | null}
 */
export function clipRectToViewport(rect, viewport) {
  const x0 = Math.max(rect.x, 0);
  const y0 = Math.max(rect.y, 0);
  const x1 = Math.min(rect.x + rect.w, viewport.width);
  const y1 = Math.min(rect.y + rect.h, viewport.height);
  const w = x1 - x0;
  const h = y1 - y0;
  if (w <= 0 || h <= 0) return null;
  return { x: x0, y: y0, w, h };
}

/**
 * @param {Rect} a
 * @param {Rect} cell
 */
function intersects(a, cell) {
  return !(
    a.x + a.w <= cell.x ||
    cell.x + cell.w <= a.x ||
    a.y + a.h <= cell.y ||
    cell.y + cell.h <= a.y
  );
}

/**
 * Largest empty rectangle on an 8×8 grid over the content area (fraction of content area).
 * Occupied cells = any panel or status-banner rect intersection (union — no double fill).
 * @param {Rect} content
 * @param {Rect[]} occupiedRects
 * @param {number} [grid]
 */
export function largestEmptyRatio(content, occupiedRects, grid = L3_GRID) {
  if (content.w <= 0 || content.h <= 0) return 1;
  const occupied = Array.from({ length: grid }, () =>
    Array.from({ length: grid }, () => false),
  );
  const cellW = content.w / grid;
  const cellH = content.h / grid;
  for (let gy = 0; gy < grid; gy++) {
    for (let gx = 0; gx < grid; gx++) {
      const cell = {
        x: content.x + gx * cellW,
        y: content.y + gy * cellH,
        w: cellW,
        h: cellH,
      };
      occupied[gy][gx] = occupiedRects.some((p) => intersects(p, cell));
    }
  }

  let best = 0;
  for (let y0 = 0; y0 < grid; y0++) {
    for (let x0 = 0; x0 < grid; x0++) {
      for (let y1 = y0; y1 < grid; y1++) {
        for (let x1 = x0; x1 < grid; x1++) {
          let empty = true;
          for (let y = y0; y <= y1 && empty; y++) {
            for (let x = x0; x <= x1; x++) {
              if (occupied[y][x]) {
                empty = false;
                break;
              }
            }
          }
          if (empty) {
            const cells = (x1 - x0 + 1) * (y1 - y0 + 1);
            best = Math.max(best, cells / (grid * grid));
          }
        }
      }
    }
  }
  return best;
}

/**
 * Pure/jsdom visibility matching Chromium `checkVisibility` ancestor coverage:
 * - display:none / visibility:hidden / effective opacity ≤ 0: walk up to
 *   `document.documentElement` (so an opacity:0 ancestor *above* the stack hides).
 * - zero-size box: walk up to and including `stack` only (skipping
 *   `display:contents`). html/body often report 0×0 without layout in jsdom.
 *
 * @param {Element} el
 * @param {Element} stack
 * @param {typeof globalThis.getComputedStyle} [getStyle]
 */
export function isEffectivelyVisibleBanner(el, stack, getStyle = globalThis.getComputedStyle) {
  if (!(el instanceof Element) || !(stack instanceof Element)) return false;
  const root = el.ownerDocument?.documentElement;
  let node = el;
  let opacityProduct = 1;
  let checkZeroSize = true;
  while (node) {
    const style = getStyle(node);
    if (style.display === "none") return false;
    if (style.visibility === "hidden") return false;
    const opacity = Number.parseFloat(style.opacity || "1");
    if (Number.isFinite(opacity)) {
      opacityProduct *= opacity;
    }
    if (opacityProduct <= 0) return false;
    if (checkZeroSize && style.display !== "contents") {
      const r = node.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) return false;
    }
    if (node === stack) checkZeroSize = false;
    if (node === root) break;
    node = node.parentElement;
  }
  return true;
}

/**
 * Collect viewport-clipped rects for visible banner elements under
 * `[data-qa="status-banner-stack"]`. Overlap is fine — L3 cells are boolean.
 *
 * Direct children of the stack (or the stack itself when childless). No
 * descendant fallback — hidden parents cannot leak occupancy via children.
 *
 * Runs in the browser (`page.evaluate`) or jsdom. Must stay closure-free so
 * Playwright can serialize it for `page.evaluate(collectStatusBannerRects)`.
 * @returns {Array<{ x: number, y: number, w: number, h: number }>}
 */
export function collectStatusBannerRects() {
  const vp = { width: window.innerWidth, height: window.innerHeight };
  /** @type {Array<{ x: number, y: number, w: number, h: number }>} */
  const out = [];

  /**
   * @param {DOMRect | { x: number, y: number, width: number, height: number }} r
   */
  function clip(r) {
    const x0 = Math.max(r.x, 0);
    const y0 = Math.max(r.y, 0);
    const x1 = Math.min(r.x + r.width, vp.width);
    const y1 = Math.min(r.y + r.height, vp.height);
    const w = x1 - x0;
    const h = y1 - y0;
    if (w <= 0 || h <= 0) return null;
    return { x: x0, y: y0, w, h };
  }

  /**
   * Ancestor-aware visibility. Prefer Element.checkVisibility (Chromium).
   * jsdom fallback: display / visibility / opacity through documentElement;
   * zero-size only through the stack (same as the Chromium zero-size walk).
   * @param {Element} el
   * @param {Element} stack
   */
  function visible(el, stack) {
    const root = document.documentElement;
    if (typeof el.checkVisibility === "function") {
      if (
        !el.checkVisibility({
          opacityProperty: true,
          visibilityProperty: true,
        })
      ) {
        return false;
      }
      // checkVisibility does not cover zero-size ancestors; walk for that.
      let node = el;
      while (node) {
        const style = getComputedStyle(node);
        if (style.display !== "contents") {
          const r = node.getBoundingClientRect();
          if (r.width <= 0 || r.height <= 0) return false;
        }
        if (node === stack) break;
        node = node.parentElement;
      }
      return true;
    }

    let node = el;
    let opacityProduct = 1;
    let checkZeroSize = true;
    while (node) {
      const style = getComputedStyle(node);
      if (style.display === "none") return false;
      if (style.visibility === "hidden") return false;
      const opacity = Number.parseFloat(style.opacity || "1");
      if (Number.isFinite(opacity)) {
        opacityProduct *= opacity;
      }
      if (opacityProduct <= 0) return false;
      if (checkZeroSize && style.display !== "contents") {
        const r = node.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) return false;
      }
      if (node === stack) checkZeroSize = false;
      if (node === root) break;
      node = node.parentElement;
    }
    return true;
  }

  const stacks = document.querySelectorAll('[data-qa="status-banner-stack"]');
  for (const stack of stacks) {
    const kids = Array.from(stack.children);
    const candidates = kids.length > 0 ? kids : [stack];
    for (const el of candidates) {
      if (!visible(el, stack)) continue;
      const clipped = clip(el.getBoundingClientRect());
      if (clipped) out.push(clipped);
    }
  }
  return out;
}

/**
 * L3 pass helper shared with the baseline report formatter.
 * @param {number} ratio
 */
export function l3Pass(ratio) {
  return ratio <= L3_EMPTY_RATIO_MAX;
}
