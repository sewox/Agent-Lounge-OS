/**
 * Token-based row heights for virtual lists.
 * Values are rem multiples of the document root font-size so UI scale
 * (al-os-ui-scale / data-ui-scale) resizes rows without a fixed px constant.
 */

/** SemanticMap page rows: 36px at the default 16px root → 2.25rem. */
export const PAGE_ROW_HEIGHT_REM = 2.25;

/** Vault dead-symbol rows (two-line card): ~52px at 16px root → 3.25rem. */
export const DEAD_SYMBOL_ROW_HEIGHT_REM = 3.25;

/**
 * Compute a pixel row height from the root font-size and a rem token.
 * Falls back to 16px when the input is missing or non-positive.
 */
export function computeTokenRowHeight(
  rootFontSizePx: number,
  tokenRem: number = PAGE_ROW_HEIGHT_REM,
): number {
  const font =
    Number.isFinite(rootFontSizePx) && rootFontSizePx > 0 ? rootFontSizePx : 16;
  const rem = Number.isFinite(tokenRem) && tokenRem > 0 ? tokenRem : PAGE_ROW_HEIGHT_REM;
  return Math.max(1, font * rem);
}

/** Read the live root font-size from a Document (or fall back to 16). */
export function readRootFontSizePx(doc: Document | null | undefined = undefined): number {
  const target = doc ?? (typeof document !== "undefined" ? document : null);
  if (!target?.documentElement) {
    return 16;
  }
  try {
    const raw = getComputedStyle(target.documentElement).fontSize;
    const parsed = Number.parseFloat(raw);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : 16;
  } catch {
    return 16;
  }
}

/**
 * Subscribe to root font-size / UI-scale attribute changes and return the
 * current token row height in CSS pixels.
 */
export function subscribeRootFontSize(onStoreChange: () => void): () => void {
  if (typeof document === "undefined" || typeof MutationObserver === "undefined") {
    return () => undefined;
  }
  const root = document.documentElement;
  const mo = new MutationObserver(onStoreChange);
  mo.observe(root, {
    attributes: true,
    attributeFilter: ["style", "class", "data-ui-scale"],
  });
  return () => mo.disconnect();
}
