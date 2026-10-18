/**
 * Shared L3 empty-region geometry for Playwright (`e2e/helpers/layout.ts`) and
 * the baseline report (`scripts/qa/generate-baseline-report.mjs`).
 *
 * L3 content area is always the full viewport band used by measureLayout
 * (contentY = 0, contentH = vp.height). Visible status banners under
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
 */

/**
 * Clip a rect to the viewport; return null if nothing remains visible.
 * @param {Rect} rect
 * @param {{ width: number, height: number }} viewport
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
 * Whether an element should contribute occupied area for L3 banners.
 * Invisible / display:none / visibility:hidden / zero-size → nothing.
 * @param {Element} el
 * @param {typeof globalThis.getComputedStyle} [getStyle]
 */
export function isVisibleOccupyingElement(el, getStyle = globalThis.getComputedStyle) {
  if (!(el instanceof Element)) return false;
  const style = getStyle(el);
  if (style.display === "none" || style.visibility === "hidden") return false;
  const opacity = Number.parseFloat(style.opacity || "1");
  if (Number.isFinite(opacity) && opacity <= 0) return false;
  const r = el.getBoundingClientRect();
  return r.width > 0 && r.height > 0;
}

/**
 * Collect viewport-clipped rects for visible banner elements under
 * `[data-qa="status-banner-stack"]`. Overlap is fine — L3 cells are boolean.
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
   * @param {Element} el
   */
  function visible(el) {
    const style = getComputedStyle(el);
    if (style.display === "none" || style.visibility === "hidden") return false;
    const opacity = Number.parseFloat(style.opacity || "1");
    if (Number.isFinite(opacity) && opacity <= 0) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  }

  const stacks = document.querySelectorAll('[data-qa="status-banner-stack"]');
  for (const stack of stacks) {
    const kids = Array.from(stack.children);
    const candidates = kids.length > 0 ? kids : [stack];
    let added = false;
    for (const el of candidates) {
      if (!visible(el)) continue;
      const clipped = clip(el.getBoundingClientRect());
      if (clipped) {
        out.push(clipped);
        added = true;
      }
    }
    // Wrapper with display:contents (or zero box) may expose no child box —
    // fall back to any visible descendant with area, then the stack itself.
    if (!added && visible(stack)) {
      const clipped = clip(stack.getBoundingClientRect());
      if (clipped) out.push(clipped);
    } else if (!added) {
      for (const el of Array.from(stack.querySelectorAll("*"))) {
        if (!visible(el)) continue;
        const clipped = clip(el.getBoundingClientRect());
        if (clipped) out.push(clipped);
      }
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
