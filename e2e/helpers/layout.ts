import type { Page } from "@playwright/test";

export type Rect = { x: number; y: number; w: number; h: number };

export type LayoutMetrics = {
  route: string;
  viewport: { width: number; height: number };
  contentW: number;
  sidebarW: number;
  panelCount: number;
  unionW: number;
  unionBottom: number;
  scrollHeight: number;
  clientHeight: number;
  pageScrolls: boolean;
  l1_widthRatio: number;
  l2_heightOk: boolean;
  l3_largestEmptyRatio: number;
  l4_centeredNarrow: boolean;
  l5_horizontalOverflow: boolean;
  l5_sidebarOverflow: boolean;
  l6_portraitStackOk: boolean | null;
  l1_pass: boolean;
  l2_pass: boolean;
  l3_pass: boolean;
  l4_pass: boolean;
  l5_pass: boolean;
  l6_pass: boolean | null;
};

function intersects(a: Rect, cell: Rect) {
  return !(a.x + a.w <= cell.x || cell.x + cell.w <= a.x || a.y + a.h <= cell.y || cell.y + cell.h <= a.y);
}

/** Largest empty rectangle on an 8x8 grid over the content area (fraction of content area). */
export function largestEmptyRatio(
  content: Rect,
  panels: Rect[],
  grid = 8,
): number {
  if (content.w <= 0 || content.h <= 0) return 1;
  const occupied = Array.from({ length: grid }, () => Array.from({ length: grid }, () => false));
  const cellW = content.w / grid;
  const cellH = content.h / grid;
  for (let gy = 0; gy < grid; gy++) {
    for (let gx = 0; gx < grid; gx++) {
      const cell: Rect = {
        x: content.x + gx * cellW,
        y: content.y + gy * cellH,
        w: cellW,
        h: cellH,
      };
      occupied[gy]![gx] = panels.some((p) => intersects(p, cell));
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
              if (occupied[y]![x]) {
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

export async function measureLayout(page: Page, route: string): Promise<LayoutMetrics> {
  // Onboarding and some routes may briefly remount; wait for any panel or main.
  await page.locator("main, [data-qa='panel']").first().waitFor({ state: "attached", timeout: 15_000 }).catch(() => undefined);
  const vp = page.viewportSize()!;
  const measured = await page.evaluate(() => {
    const sidebarEl = document.querySelector('[data-qa="sidebar"]') as HTMLElement | null;
    const mainEl = document.querySelector("main") as HTMLElement | null;
    const panelEls = Array.from(document.querySelectorAll('main [data-qa="panel"]')) as HTMLElement[];
    const sidebar = sidebarEl?.getBoundingClientRect() ?? null;
    const main = mainEl?.getBoundingClientRect() ?? null;
    const panels = panelEls.map((e) => {
      const r = e.getBoundingClientRect();
      return { x: r.x, y: r.y, w: r.width, h: r.height };
    });
    const scrollWidth = document.documentElement.scrollWidth;
    const innerWidth = window.innerWidth;
    const scrollHeight = Math.max(
      document.documentElement.scrollHeight,
      mainEl?.scrollHeight ?? 0,
    );
    const clientHeight = window.innerHeight;
    const mainScrolls = mainEl ? mainEl.scrollHeight > mainEl.clientHeight + 2 : false;

    // Sidebar children overflow (e.g. + New Node) — also compare scrollWidth.
    let sidebarOverflow = false;
    if (sidebarEl && sidebar) {
      if (sidebarEl.scrollWidth > sidebarEl.clientWidth + 1) {
        sidebarOverflow = true;
      }
      for (const child of Array.from(sidebarEl.querySelectorAll("button, a, span")) as HTMLElement[]) {
        const r = child.getBoundingClientRect();
        if (r.width === 0 && r.height === 0) continue;
        if (r.right > sidebar.right + 0.5 || r.left < sidebar.left - 0.5) {
          sidebarOverflow = true;
          break;
        }
      }
    }

    return {
      sidebarW: sidebar?.width ?? 0,
      main: main ? { x: main.x, y: main.y, w: main.width, h: main.height } : null,
      panels,
      scrollWidth,
      innerWidth,
      scrollHeight,
      clientHeight,
      mainScrolls,
      sidebarOverflow,
    };
  });

  const contentW = vp.width - measured.sidebarW;
  const contentX = measured.main?.x ?? measured.sidebarW;
  const contentY = 0;
  const contentH = vp.height;
  const panels = measured.panels.filter((p) => p.w > 0 && p.h > 0);

  const union =
    panels.length === 0
      ? { x0: contentX, x1: contentX, y1: 0 }
      : {
          x0: Math.min(...panels.map((p) => p.x)),
          x1: Math.max(...panels.map((p) => p.x + p.w)),
          y1: Math.max(...panels.map((p) => p.y + p.h)),
        };

  const unionW = union.x1 - union.x0;
  const l1_widthRatio = contentW > 0 ? unionW / contentW : 0;
  const pageScrolls = measured.mainScrolls || measured.scrollHeight > measured.clientHeight + 2;
  const l2_heightOk = union.y1 >= 0.85 * vp.height || pageScrolls;

  const contentRect: Rect = { x: contentX, y: contentY, w: contentW, h: contentH };
  const l3_largestEmptyRatio = largestEmptyRatio(contentRect, panels);

  // L4: single panel centered with max-width and width < 70% of content
  let l4_centeredNarrow = false;
  if (panels.length === 1) {
    const p = panels[0]!;
    const leftGap = p.x - contentX;
    const rightGap = contentX + contentW - (p.x + p.w);
    const centered = Math.abs(leftGap - rightGap) < contentW * 0.08 && leftGap > contentW * 0.1;
    if (centered && p.w < 0.7 * contentW) {
      l4_centeredNarrow = true;
    }
  }

  const l5_horizontalOverflow = measured.scrollWidth > measured.innerWidth + 1;
  const l5_sidebarOverflow = measured.sidebarOverflow;

  const isPortrait = vp.height > vp.width;
  let l6_portraitStackOk: boolean | null = null;
  if (isPortrait) {
    l6_portraitStackOk =
      panels.length === 0 ||
      panels.every((p) => p.w >= 0.95 * contentW - 1);
  }

  const l1_pass = l1_widthRatio >= 0.85;
  const l2_pass = l2_heightOk;
  const l3_pass = l3_largestEmptyRatio <= 0.15;
  const l4_pass = !l4_centeredNarrow;
  const l5_pass = !l5_horizontalOverflow && !l5_sidebarOverflow;
  const l6_pass = l6_portraitStackOk;

  return {
    route,
    viewport: { width: vp.width, height: vp.height },
    contentW,
    sidebarW: measured.sidebarW,
    panelCount: panels.length,
    unionW,
    unionBottom: union.y1,
    scrollHeight: measured.scrollHeight,
    clientHeight: measured.clientHeight,
    pageScrolls,
    l1_widthRatio,
    l2_heightOk,
    l3_largestEmptyRatio,
    l4_centeredNarrow,
    l5_horizontalOverflow,
    l5_sidebarOverflow,
    l6_portraitStackOk,
    l1_pass,
    l2_pass,
    l3_pass,
    l4_pass,
    l5_pass,
    l6_pass,
  };
}

export function formatLayoutFailure(m: LayoutMetrics): string {
  const fails: string[] = [];
  if (!m.l1_pass) fails.push(`L1 width ${(m.l1_widthRatio * 100).toFixed(1)}% < 85%`);
  if (!m.l2_pass) fails.push(`L2 height bottom=${m.unionBottom.toFixed(0)} < 85% of ${m.viewport.height}`);
  if (!m.l3_pass) fails.push(`L3 empty ${(m.l3_largestEmptyRatio * 100).toFixed(1)}% > 15%`);
  if (!m.l4_pass) fails.push(`L4 centered narrow panel`);
  if (!m.l5_pass) {
    if (m.l5_horizontalOverflow) fails.push(`L5 horizontal overflow`);
    if (m.l5_sidebarOverflow) fails.push(`L5 sidebar child overflow`);
  }
  if (m.l6_pass === false) fails.push(`L6 portrait panels not >=95% content width`);
  return fails.join("; ") || "ok";
}
