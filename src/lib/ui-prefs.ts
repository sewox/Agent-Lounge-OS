import type { ToolQuota } from "@/lib/lounge";

export const UI_SCALE_KEY = "al-os-ui-scale";
export const PANEL_COLLAPSE_KEY = "al-os-panel-collapsed";

export const UI_SCALES = [0.9, 1, 1.15, 1.3] as const;
export type UiScale = (typeof UI_SCALES)[number];

export const DEFAULT_UI_SCALE: UiScale = 1;

/** Criticality rank: higher = more urgent for the dashboard mini-card. */
export function quotaCriticality(row: ToolQuota): number {
  const percent = row.percent ?? (row.exhausted ? 100 : 0);
  let score = percent;
  if (row.exhausted || row.tone === "amber") {
    score += 100;
  } else if (row.tone === "warn") {
    score += 50;
  }
  return score;
}

/** Top N quotas by usage / alert state (highest first). */
export function selectCriticalQuotas(quotas: ToolQuota[], limit = 3): ToolQuota[] {
  return [...quotas]
    .sort((a, b) => {
      const diff = quotaCriticality(b) - quotaCriticality(a);
      if (diff !== 0) {
        return diff;
      }
      return a.tool.localeCompare(b.tool);
    })
    .slice(0, Math.max(0, limit));
}

export function isUiScale(value: unknown): value is UiScale {
  return typeof value === "number" && (UI_SCALES as readonly number[]).includes(value);
}

export function parseUiScale(raw: string | null | undefined): UiScale {
  if (raw == null || raw === "") {
    return DEFAULT_UI_SCALE;
  }
  const n = Number(raw);
  if (isUiScale(n)) {
    return n;
  }
  return DEFAULT_UI_SCALE;
}

export function nextUiScale(current: UiScale, direction: 1 | -1): UiScale {
  const idx = UI_SCALES.indexOf(current);
  const next = Math.min(UI_SCALES.length - 1, Math.max(0, idx + direction));
  return UI_SCALES[next] ?? DEFAULT_UI_SCALE;
}

export function readStoredUiScale(storage?: Pick<Storage, "getItem"> | null): UiScale {
  try {
    return parseUiScale(storage?.getItem(UI_SCALE_KEY));
  } catch {
    return DEFAULT_UI_SCALE;
  }
}

export function writeStoredUiScale(
  scale: UiScale,
  storage?: Pick<Storage, "setItem"> | null,
): void {
  try {
    storage?.setItem(UI_SCALE_KEY, String(scale));
  } catch {
    /* ignore quota / private mode */
  }
}

export type PanelCollapseMap = Record<string, boolean>;

export function readPanelCollapsed(
  storage?: Pick<Storage, "getItem"> | null,
): PanelCollapseMap {
  try {
    const raw = storage?.getItem(PANEL_COLLAPSE_KEY);
    if (!raw) {
      return {};
    }
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {};
    }
    const out: PanelCollapseMap = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (typeof value === "boolean") {
        out[key] = value;
      }
    }
    return out;
  } catch {
    return {};
  }
}

export function writePanelCollapsed(
  map: PanelCollapseMap,
  storage?: Pick<Storage, "setItem"> | null,
): void {
  try {
    storage?.setItem(PANEL_COLLAPSE_KEY, JSON.stringify(map));
  } catch {
    /* ignore */
  }
}

export function isPanelCollapsed(
  id: string,
  map: PanelCollapseMap,
  defaultCollapsed = false,
): boolean {
  return id in map ? Boolean(map[id]) : defaultCollapsed;
}
