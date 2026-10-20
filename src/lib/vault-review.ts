/**
 * Pure helpers for Vault Review ordering, filters, and counter consistency.
 * Keep in sync with Rust `list_review_queue` / `count_review_stats`.
 */

export type ReviewStatId = "pending" | "hygiene" | "approved_today" | "rejected";

export type ReviewableExperience = {
  id: string;
  agent: string;
  project_id: string;
  adr_summary: string;
  outcome: string;
  tags: string[];
  created_at: string;
  status?: string;
  reviewed?: boolean;
  archived_by?: string | null;
  title?: string;
  original_content?: string | null;
  topic?: string;
  solution_summary?: string;
};

export const HYGIENE_PLACEHOLDERS = [
  "<device-ip>",
  "<redacted-token>",
  "<device-serial>",
  "<home>",
] as const;

export function textHasHygieneMask(text: string | null | undefined): boolean {
  if (!text) return false;
  return HYGIENE_PLACEHOLDERS.some((p) => text.includes(p));
}

export function experienceHasHygieneMask(row: ReviewableExperience): boolean {
  return (
    textHasHygieneMask(row.adr_summary) ||
    textHasHygieneMask(row.title) ||
    textHasHygieneMask(row.original_content) ||
    textHasHygieneMask(row.topic) ||
    textHasHygieneMask(row.solution_summary) ||
    (row.tags ?? []).some((tag) => textHasHygieneMask(tag))
  );
}

/** Hygiene-flagged first, then newest `created_at`. */
export function sortReviewQueue<T extends ReviewableExperience>(rows: T[]): T[] {
  return [...rows].sort((a, b) => {
    const ha = experienceHasHygieneMask(a) ? 0 : 1;
    const hb = experienceHasHygieneMask(b) ? 0 : 1;
    if (ha !== hb) return ha - hb;
    return b.created_at.localeCompare(a.created_at);
  });
}

export type ReviewFilterBar = {
  projectId: string | null;
  agent: string | null;
  outcome: string | null;
  hygieneOnly: boolean;
};

export function applyReviewFilters<T extends ReviewableExperience>(
  rows: T[],
  filters: ReviewFilterBar,
): T[] {
  return rows.filter((row) => {
    if (filters.projectId && row.project_id !== filters.projectId) return false;
    if (filters.agent && row.agent !== filters.agent) return false;
    if (filters.outcome && row.outcome !== filters.outcome) return false;
    if (filters.hygieneOnly && !experienceHasHygieneMask(row)) return false;
    return true;
  });
}

export type ReviewStatsCounts = {
  pending: number;
  hygiene: number;
  approved_today: number;
  rejected: number;
};

/**
 * Pending must equal `count_unreviewed_experiences` (active + reviewed=false).
 * Callers pass the canonical unreviewed count so badge / strip / list stay aligned.
 */
export function assertCounterConsistency(
  unreviewedCount: number,
  stats: Pick<ReviewStatsCounts, "pending">,
): boolean {
  return unreviewedCount === stats.pending;
}

/** Same-band ordering: reviewed before unreviewed (stable within band by created_at). */
export function sortSearchByReviewedBand<T extends { reviewed?: boolean; created_at: string }>(
  rows: T[],
): T[] {
  return [...rows].sort((a, b) => {
    const ra = a.reviewed === false ? 1 : 0;
    const rb = b.reviewed === false ? 1 : 0;
    if (ra !== rb) return ra - rb;
    return b.created_at.localeCompare(a.created_at);
  });
}
