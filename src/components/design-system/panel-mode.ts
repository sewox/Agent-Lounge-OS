/**
 * Container-driven panel modes for the 1/2/3-panel grid.
 * Breakpoints use container width (not device); portrait uses aspect ratio.
 */

export type PanelMode = "one" | "two" | "three" | "portrait-stack";

/** Narrow: single panel + drawers. */
export const PANEL_NARROW_MAX = 1200;
/** Medium upper bound (exclusive of wide). */
export const PANEL_MEDIUM_MAX = 1600;

/**
 * Resolve layout mode from the grid container's box.
 * Portrait (aspect &lt; 1) always stacks list above detail with a splitter.
 */
export function resolvePanelMode(width: number, height: number): PanelMode {
  if (!(width > 0) || !(height > 0)) {
    return "one";
  }
  if (width / height < 1) {
    return "portrait-stack";
  }
  if (width < PANEL_NARROW_MAX) {
    return "one";
  }
  if (width < PANEL_MEDIUM_MAX) {
    return "two";
  }
  return "three";
}

export function modeShowsDetailInline(mode: PanelMode): boolean {
  return mode === "two" || mode === "three" || mode === "portrait-stack";
}

export function modeShowsInspectorInline(mode: PanelMode): boolean {
  return mode === "three";
}
