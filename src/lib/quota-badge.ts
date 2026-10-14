import { AMBER_THRESHOLD, type ToolQuota } from "@/lib/lounge";

/** Percent at or above this (or exhausted) uses the error badge tone. */
export const QUOTA_BADGE_ERROR_THRESHOLD = 95;

/** Rotation interval for multi-tool amber badge (ms). */
export const QUOTA_BADGE_ROTATE_MS = 4000;

export type QuotaBadgeTone = "ok" | "amber" | "error";

/** Human-facing tool name for the header badge (data string — render with lang="en"). */
export function quotaBadgeName(row: ToolQuota): string {
  const name = (row.tool || row.label || row.id).trim();
  return name || row.id;
}

function tieBreak(a: ToolQuota, b: ToolQuota): number {
  const labelCmp = (a.label || a.tool || a.id).localeCompare(
    b.label || b.tool || b.id,
    "en",
  );
  if (labelCmp !== 0) {
    return labelCmp;
  }
  return a.id.localeCompare(b.id, "en");
}

/**
 * Amber-threshold quota rows for the header badge, highest percent first.
 * Stable tie-break by label then id.
 */
export function selectAmberQuotaRows(quotas: readonly ToolQuota[]): ToolQuota[] {
  return quotas
    .filter((row) => (row.percent ?? 0) >= AMBER_THRESHOLD)
    .slice()
    .sort((a, b) => {
      const diff = (b.percent ?? 0) - (a.percent ?? 0);
      if (diff !== 0) {
        return diff;
      }
      return tieBreak(a, b);
    });
}

export function quotaBadgeTone(row: ToolQuota): QuotaBadgeTone {
  const percent = row.percent ?? (row.exhausted ? 100 : 0);
  if (row.exhausted || percent >= QUOTA_BADGE_ERROR_THRESHOLD) {
    return "error";
  }
  if (percent >= AMBER_THRESHOLD) {
    return "amber";
  }
  return "ok";
}

export function quotaBadgePercent(row: ToolQuota): number {
  return Math.round(row.percent ?? (row.exhausted ? 100 : 0));
}

/** Signature used to reset rotation when the amber set changes. */
export function amberRowsSignature(rows: readonly ToolQuota[]): string {
  return rows.map((row) => `${row.id}:${quotaBadgePercent(row)}`).join("|");
}

export function clampBadgeIndex(index: number, length: number): number {
  if (length <= 0) {
    return 0;
  }
  if (index < 0 || index >= length) {
    return 0;
  }
  return index;
}
