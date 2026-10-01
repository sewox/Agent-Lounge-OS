import type { ExperienceOutcome, LoungeExperience, ProjectSummary, SemanticMap } from "@/lib/lounge";

const INTERNAL_LINE_PATTERNS = [
  /^memory_bridge hata:/i,
  /^repo_path çözümlenemedi/i,
  /^##\s*Cross-Project Memory/i,
  /^###\s*Tecrübeler/i,
  /^```/,
  /^-\s*\[e2e-verify/i,
];

const INTERNAL_INLINE_PATTERNS = [
  /##\s*Cross-Project Memory/gi,
  /###\s*Tecrübeler/gi,
  /memory_bridge hata:/gi,
  /repo_path çözümlenemedi/gi,
  /```[\s\S]*?```/g,
];

/** EX-15: user-facing log line — no raw markdown dumps or internal TR errors. */
export function formatExperienceLogSummary(raw: string, maxLen = 180): string {
  const lines = raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !INTERNAL_LINE_PATTERNS.some((re) => re.test(line)));

  let text = lines.join(" ");
  for (const re of INTERNAL_INLINE_PATTERNS) {
    text = text.replace(re, " ");
  }
  text = text.replace(/\s+/g, " ").trim();

  if (!text) {
    const fallback = raw.match(
      /(?:Experience ADR|Indexed|solved|routing|documented|fallback)[^.!\n]{8,120}/i,
    );
    text = fallback?.[0]?.trim() ?? "Experience recorded.";
  }

  if (text.length > maxLen) {
    return `${text.slice(0, maxLen - 1).trim()}…`;
  }
  return text;
}

/** PATH-01: preserve `/`, `\`, and drive letters for display. */
export function formatDisplayPath(path: string | null | undefined): string {
  if (!path?.trim()) {
    return "—";
  }
  return path.trim();
}

export function acceptsCrossPlatformPath(path: string): boolean {
  const trimmed = path.trim();
  if (!trimmed) {
    return false;
  }
  return (
    /^[A-Za-z]:[\\/]/.test(trimmed) ||
    trimmed.startsWith("/") ||
    trimmed.startsWith("\\\\") ||
    /[/\\]/.test(trimmed)
  );
}

export function isExperienceArchived(row: LoungeExperience): boolean {
  return row.status === "archived";
}

export function sortExperiencesForDisplay(rows: LoungeExperience[]): LoungeExperience[] {
  return rows.slice().sort((left, right) => {
    const pinDelta = Number(right.is_pinned) - Number(left.is_pinned);
    if (pinDelta !== 0) {
      return pinDelta;
    }
    return right.created_at.localeCompare(left.created_at);
  });
}

/** EX-14: backend COUNT totals — never LIMIT-shaped list lengths. */
export function resolveGraphTotals(input: {
  projects: ProjectSummary[];
  semanticMap: SemanticMap;
}): { nodes: number; edges: number; files: number } {
  if (input.projects.length > 0) {
    return {
      nodes: input.projects.reduce((sum, row) => sum + (row.nodes ?? 0), 0),
      edges: input.projects.reduce((sum, row) => sum + (row.edges ?? 0), 0),
      files: input.projects.reduce((sum, row) => sum + (row.files ?? 0), 0),
    };
  }
  if (input.semanticMap.projects.length > 0) {
    return {
      nodes: input.semanticMap.projects.reduce((sum, row) => sum + row.node_count, 0),
      edges: input.semanticMap.projects.reduce((sum, row) => sum + row.edge_count, 0),
      files: input.semanticMap.projects.reduce((sum, row) => sum + row.files, 0),
    };
  }
  return { nodes: 0, edges: 0, files: 0 };
}

export type ExperienceUpdatePatch = {
  adr_summary?: string;
  tags?: string[];
  outcome?: ExperienceOutcome;
  project_id?: string;
};

export function experienceMatchesPatch(
  current: LoungeExperience,
  patch: ExperienceUpdatePatch,
): LoungeExperience {
  return {
    ...current,
    adr_summary: patch.adr_summary ?? current.adr_summary,
    tags: patch.tags ?? current.tags,
    outcome: patch.outcome ?? current.outcome,
    project_id: patch.project_id ?? current.project_id,
    updated_at: new Date().toISOString(),
  };
}
