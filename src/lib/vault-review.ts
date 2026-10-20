/**
 * Pure helpers for Vault Review ordering, filters, and counter consistency.
 * Keep in sync with Rust `list_review_queue` / `count_unreviewed_experiences`
 * and UD-5 `REVIEW_SCORE_BAND` (docs/mcp.md).
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

/** Must match Rust `REVIEW_SCORE_BAND` in experience_governance.rs / docs/mcp.md. */
export const REVIEW_SCORE_BAND = 0.05;

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

/**
 * Strip absolute home roots before any review content reaches the DOM.
 * Raw pre-hygiene paths must never appear in the review view.
 */
export function redactHomePaths(text: string): string {
  return text
    .replace(/\/Users\/[^/\s]+/gi, "<home>")
    .replace(/\/home\/[^/\s]+/gi, "<home>")
    .replace(/C:\\Users\\[^\\\s]+/gi, "<home>");
}

/** Hygiene-masked body for the inspector (once). Never returns raw home paths. */
export function maskedReviewContent(row: ReviewableExperience): string {
  const raw = (row.adr_summary || row.title || "").trim();
  return redactHomePaths(raw);
}

/** Drop original_content so pre-hygiene text cannot leak into React trees. */
export function sanitizeReviewRow<T extends ReviewableExperience>(row: T): T {
  return {
    ...row,
    original_content: null,
    adr_summary: redactHomePaths(row.adr_summary ?? ""),
    title: row.title != null ? redactHomePaths(row.title) : row.title,
  };
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

/**
 * UD-5: relevance primary; within REVIEW_SCORE_BAND, reviewed before unreviewed.
 * Callers supply normalized scores in [0, 1] (same contract as Rust lexical_search_score).
 */
export function sortSearchByReviewedBand<
  T extends { reviewed?: boolean; created_at: string; score?: number },
>(rows: T[]): T[] {
  return [...rows].sort((a, b) => {
    const sa = a.score ?? (a.reviewed === false ? 0.5 : 0.5);
    const sb = b.score ?? (b.reviewed === false ? 0.5 : 0.5);
    if (Math.abs(sa - sb) > REVIEW_SCORE_BAND) {
      return sb - sa;
    }
    const ra = a.reviewed === false ? 1 : 0;
    const rb = b.reviewed === false ? 1 : 0;
    if (ra !== rb) return ra - rb;
    return b.created_at.localeCompare(a.created_at);
  });
}
