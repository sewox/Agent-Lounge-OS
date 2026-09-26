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
  /** Min interior content fill among tall panels (rows vs stretched frame). */
  l2_minInteriorFill: number;
  l2_sparseInterior: boolean;
  l3_largestEmptyRatio: number;
  l4_centeredNarrow: boolean;
  /** Single panel narrower than 70% content (left-aligned or centered) — live G4. */
  l4_narrowSinglePanel: boolean;
  l5_horizontalOverflow: boolean;
  l5_sidebarOverflow: boolean;
  /** + New Node overlaps AL-OS CORE title (live G1). */
  l5_newNodeOverlap: boolean;
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

function rectsOverlap(a: Rect, b: Rect) {
  return !(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y);
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

    // Interior fill: deepest visible child bottom vs panel frame (catches stretched empty frames).
    const panelInteriorFills = panelEls.map((panel) => {
      const pr = panel.getBoundingClientRect();
      if (pr.height < 8) return 1;
      let maxBottom = pr.y;
      const kids = panel.querySelectorAll(
        "li, tr, [class*='truncate'], button, a, p, pre, code, h1, h2, h3, h4, span.font-mono, span.font-body",
      );
      for (const kid of Array.from(kids) as HTMLElement[]) {
        const r = kid.getBoundingClientRect();
        if (r.width < 2 || r.height < 2) continue;
        if (r.bottom > maxBottom) maxBottom = r.bottom;
      }
      const fill = Math.min(1, Math.max(0, (maxBottom - pr.y) / pr.height));
      return fill;
    });

    const scrollWidth = Math.max(
      document.documentElement.scrollWidth,
      mainEl?.scrollWidth ?? 0,
      ...panelEls.map((p) => p.scrollWidth),
    );
    const innerWidth = window.innerWidth;
    const scrollHeight = Math.max(
      document.documentElement.scrollHeight,
      mainEl?.scrollHeight ?? 0,
    );
    const clientHeight = window.innerHeight;
    const mainScrolls = mainEl ? mainEl.scrollHeight > mainEl.clientHeight + 2 : false;

    let sidebarOverflow = false;
    let newNodeOverlap = false;
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
      // G1: + New Node overlapping AL-OS CORE title.
      const title = Array.from(sidebarEl.querySelectorAll("div, span")).find((el) =>
        /AL-OS\s*CORE/i.test(el.textContent || ""),
      ) as HTMLElement | undefined;
      const newNode = Array.from(sidebarEl.querySelectorAll("button")).find((el) =>
        /\+\s*New Node/i.test(el.textContent || ""),
      ) as HTMLElement | undefined;
      if (title && newNode) {
        const a = title.getBoundingClientRect();
        const b = newNode.getBoundingClientRect();
        const overlap = !(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top);
        // Also catch when New Node sits over the title's horizontal band (clipped text).
        const sameRow = Math.abs(a.top - b.top) < Math.max(a.height, b.height);
        if (overlap || (sameRow && b.left < a.right - 4 && b.right > a.left)) {
          newNodeOverlap = true;
        }
      }
    }

    return {
      sidebarW: sidebar?.width ?? 0,
      main: main ? { x: main.x, y: main.y, w: main.width, h: main.height } : null,
      panels,
      panelInteriorFills,
      scrollWidth,
      innerWidth,
      scrollHeight,
      clientHeight,
      mainScrolls,
      sidebarOverflow,
      newNodeOverlap,
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

  // Sparse interior: tall panels (≥50% viewport) whose content ends before 45% of frame height.
  let l2_minInteriorFill = 1;
  let l2_sparseInterior = false;
  for (let i = 0; i < panels.length; i++) {
    const p = panels[i]!;
    const fill = measured.panelInteriorFills[i] ?? 1;
    if (p.h >= 0.5 * vp.height) {
      l2_minInteriorFill = Math.min(l2_minInteriorFill, fill);
      if (fill < 0.45) l2_sparseInterior = true;
    }
  }

  const contentRect: Rect = { x: contentX, y: contentY, w: contentW, h: contentH };
  const l3_largestEmptyRatio = largestEmptyRatio(contentRect, panels);

  // L4: narrow single panel — centered (classic) OR left-aligned unused width (live G4).
  let l4_centeredNarrow = false;
  let l4_narrowSinglePanel = false;
  if (panels.length === 1) {
    const p = panels[0]!;
    const leftGap = p.x - contentX;
    const rightGap = contentX + contentW - (p.x + p.w);
    const centered = Math.abs(leftGap - rightGap) < contentW * 0.08 && leftGap > contentW * 0.1;
    if (p.w < 0.7 * contentW) {
      l4_narrowSinglePanel = true;
      if (centered) l4_centeredNarrow = true;
    }
  }

  const l5_horizontalOverflow = measured.scrollWidth > measured.innerWidth + 1;
  const l5_sidebarOverflow = measured.sidebarOverflow;
  const l5_newNodeOverlap = measured.newNodeOverlap;

  const isPortrait = vp.height > vp.width;
  let l6_portraitStackOk: boolean | null = null;
  if (isPortrait) {
    l6_portraitStackOk =
      panels.length === 0 ||
      panels.every((p) => p.w >= 0.95 * contentW - 1);
  }

  const l1_pass = l1_widthRatio >= 0.85;
  const l2_pass = l2_heightOk && !l2_sparseInterior;
  const l3_pass = l3_largestEmptyRatio <= 0.15;
  const l4_pass = !l4_centeredNarrow && !l4_narrowSinglePanel;
  const l5_pass = !l5_horizontalOverflow && !l5_sidebarOverflow && !l5_newNodeOverlap;
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
    l2_minInteriorFill,
    l2_sparseInterior,
    l3_largestEmptyRatio,
    l4_centeredNarrow,
    l4_narrowSinglePanel,
    l5_horizontalOverflow,
    l5_sidebarOverflow,
    l5_newNodeOverlap,
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
  if (!m.l2_heightOk) fails.push(`L2 height bottom=${m.unionBottom.toFixed(0)} < 85% of ${m.viewport.height}`);
  if (m.l2_sparseInterior) {
    fails.push(`L2 sparse interior fill ${(m.l2_minInteriorFill * 100).toFixed(0)}% < 45% (stretched frame)`);
  }
  if (!m.l3_pass) fails.push(`L3 empty ${(m.l3_largestEmptyRatio * 100).toFixed(1)}% > 15%`);
  if (m.l4_narrowSinglePanel) fails.push(`L4 narrow single panel (${(m.l1_widthRatio * 100).toFixed(0)}% width)`);
  else if (m.l4_centeredNarrow) fails.push(`L4 centered narrow panel`);
  if (!m.l5_pass) {
    if (m.l5_horizontalOverflow) fails.push(`L5 horizontal overflow`);
    if (m.l5_sidebarOverflow) fails.push(`L5 sidebar child overflow`);
    if (m.l5_newNodeOverlap) fails.push(`L5 + New Node overlaps AL-OS CORE`);
  }
  if (m.l6_pass === false) fails.push(`L6 portrait panels not >=95% content width`);
  return fails.join("; ") || "ok";
}

// Silence unused helper warning in some bundlers.
void rectsOverlap;
