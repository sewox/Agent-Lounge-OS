/** Health-UI-2: agent status mapping, path masking, dead-symbol filters. */

import type { AgentMcpStatus, AgentHealthEntry } from "@/lib/lounge";
import type { StatusTone } from "@/components/design-system";
import {
  approvalKindIcon,
  SOURCE_UNVERIFIED_APPROVAL_ICON as APPROVAL_SOURCE_UNVERIFIED_ICON,
} from "@/lib/approvals-chrome";

export const AGENT_MCP_STATUSES: readonly AgentMcpStatus[] = [
  "ok",
  "missing",
  "stale",
  "hash_mismatch",
  "non_canonical",
  "unreadable",
  "not_installed",
  "unverified",
] as const;

/**
 * Approval chrome icon for `source_unverified` — same glyph the shell banner
 * renders (`approvalKindIcon`), never the remote-link `link-off` agent icon.
 */
export const SOURCE_UNVERIFIED_APPROVAL_ICON = APPROVAL_SOURCE_UNVERIFIED_ICON;

/** i18n keys under `health.status.*` — one distinct label per status. */
export function statusLabelKey(status: AgentMcpStatus): string {
  return `status.${status}`;
}

/** Map report severity → StatusPill tone (ok / info / warning). */
export function severityToTone(severity: string): StatusTone {
  const s = severity.trim().toLowerCase();
  if (s === "ok") return "ok";
  if (s === "info") return "info";
  if (s === "warning" || s === "warn") return "warn";
  return "neutral";
}

/** Distinct icon name for agent-MCP status (must differ from approval chrome). */
export function statusIconName(status: AgentMcpStatus): string {
  switch (status) {
    case "ok":
      return "health";
    case "missing":
    case "not_installed":
      return "folder";
    case "stale":
      return "tune";
    case "hash_mismatch":
      return "chip";
    case "non_canonical":
    case "unreadable":
      return "rule";
    case "unverified":
      return "link-off";
    default:
      return "health";
  }
}

/** True when a string looks like a filesystem path (not a URL or free text). */
export function isPathLike(raw: string): boolean {
  const v = raw.trim();
  if (!v) return false;
  if (v.startsWith("~/") || v === "~") return true;
  if (/^[A-Za-z]:[\\/]/.test(v)) return true;
  if (v.startsWith("/") && !v.startsWith("//")) return true;
  return false;
}

/** Home path sits after `scheme://host[:port]` — leave URL path segments alone. */
function isUrlPathContext(text: string, index: number): boolean {
  const before = text.slice(0, index);
  return /[a-z][a-z0-9+.-]*:\/\/[^\s]*$/i.test(before);
}

/** Allow mid-string mask only after start / whitespace / `= : " ' ( ,`. */
function isBoundaryBefore(text: string, index: number): boolean {
  if (index === 0) return true;
  return /[\s=:\"'(,]/.test(text.charAt(index - 1));
}

/**
 * Second-line defence (HO-4): mask home roots in path fields **and** free text.
 * Boundary-aware mid-string match for `/Users/<n>`, `/home/<n>`, `X:\Users\<n>`.
 * Also masks `file:///Users|home|C:/Users/...` URI homes (journal scrub parity).
 * Does **not** rewrite HTTP(S) URL path segments (`http://host/home/...`).
 */
export function maskHomePathForUi(raw: string | null | undefined): string {
  if (raw == null || raw === "") return "";
  if (raw.startsWith("~/") || raw === "~") {
    return raw.replace(/\\/g, "/");
  }

  // file:// home URIs — treat as filesystem paths, not http URL path segments.
  let out = raw.replace(
    /file:\/\/\/[A-Za-z]:[\\/]+Users[\\/]+[^\\/\s"']+/gi,
    "file://~",
  );
  out = out.replace(/file:\/\/\/Users\/[^/\s"']+/gi, "file://~");
  out = out.replace(/file:\/\/\/home\/[^/\s"']+/gi, "file://~");

  const patterns: RegExp[] = [
    /[A-Za-z]:[\\/]+Users[\\/]+[^\\/\s"']+/gi,
    /\/Users\/[^/\s"']+/gi,
    /\/home\/[^/\s"']+/gi,
  ];

  for (const re of patterns) {
    out = out.replace(re, (match, offset: number) => {
      if (typeof offset !== "number") return match;
      if (isUrlPathContext(out, offset)) return match;
      if (!isBoundaryBefore(out, offset)) return match;
      return "~";
    });
  }

  out = out.replace(/\\/g, "/");
  // After replacing a Windows home root, `~\AppData` → `~/AppData`.
  out = out.replace(/~(?![/\s]|$)/g, "~/");
  out = out.replace(/~\/+/g, "~/");
  return out;
}

/** True when a display string still contains an absolute home leak. */
export function containsUnmaskedHome(text: string): boolean {
  return (
    /\/Users\//i.test(text) ||
    /\/home\//i.test(text) ||
    /[A-Za-z]:[\\/]+Users[\\/]/i.test(text)
  );
}

/** Mask path fields and free-text detail (second-line HO-4 defence). */
export function maskAgentEntryForUi(entry: AgentHealthEntry): AgentHealthEntry {
  return {
    ...entry,
    config_path:
      entry.config_path != null ? maskHomePathForUi(entry.config_path) : entry.config_path,
    command: entry.command != null ? maskHomePathForUi(entry.command) : entry.command,
    detail: entry.detail != null ? maskHomePathForUi(entry.detail) : entry.detail,
  };
}

/** Re-export for tests that assert against the live approval icon wiring. */
export { approvalKindIcon };

export type DeadConfidenceFilter = "all" | "Kritik" | "Potansiyel";
export type DeadCategoryFilter = "all" | "unused" | "broken" | "partial";

export type DeadSymbolFilterInput = {
  query: string;
  category: DeadCategoryFilter;
  confidence: DeadConfidenceFilter;
  projectId?: string | null;
  partial?: boolean;
};

export function normalizeConfidence(raw: string | null | undefined): "Kritik" | "Potansiyel" {
  if (raw === "Kritik") return "Kritik";
  return "Potansiyel";
}

export function countDeadByCategory<T extends { kind?: string | null; severity?: string | null }>(
  symbols: T[],
): { total: number; unused: number; broken: number; kritik: number; potansiyel: number } {
  let unused = 0;
  let broken = 0;
  let kritik = 0;
  let potansiyel = 0;
  for (const s of symbols) {
    if (s.kind === "unused") unused += 1;
    else if (s.kind === "broken") broken += 1;
    if (normalizeConfidence(s.severity) === "Kritik") kritik += 1;
    else potansiyel += 1;
  }
  return { total: symbols.length, unused, broken, kritik, potansiyel };
}

export function filterDeadSymbolsUi<
  T extends {
    name: string;
    kind?: string | null;
    file?: string | null;
    detail?: string | null;
    project_id?: string | null;
    last_ref?: string | null;
    severity?: string | null;
    rule?: string | null;
  },
>(symbols: T[], input: DeadSymbolFilterInput): T[] {
  const q = input.query.trim().toLowerCase();
  return symbols.filter((symbol) => {
    if (input.projectId && symbol.project_id && symbol.project_id !== input.projectId) {
      return false;
    }
    if (input.category === "unused" || input.category === "broken") {
      if (symbol.kind !== input.category) return false;
    }
    if (input.confidence !== "all") {
      if (normalizeConfidence(symbol.severity) !== input.confidence) return false;
    }
    if (!q) return true;
    const hay = [
      symbol.name,
      symbol.kind ?? "",
      symbol.file ?? "",
      symbol.detail ?? "",
      symbol.project_id ?? "",
      symbol.last_ref ?? "",
      symbol.severity ?? "",
      symbol.rule ?? "",
    ]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  });
}

export function deadSymbolReason(
  symbol: {
    kind?: string | null;
    severity?: string | null;
    detail?: string | null;
    rule?: string | null;
  },
  labels: { unused: string; broken: string; kritik: string; potansiyel: string; fallback: string },
): string {
  const parts: string[] = [];
  if (symbol.kind === "unused") parts.push(labels.unused);
  else if (symbol.kind === "broken") parts.push(labels.broken);
  const conf = normalizeConfidence(symbol.severity);
  parts.push(conf === "Kritik" ? labels.kritik : labels.potansiyel);
  const detail = (symbol.detail ?? symbol.rule ?? "").trim();
  if (detail) parts.push(detail);
  return parts.length > 0 ? parts.join(" · ") : labels.fallback;
}

export function isScanStale(
  meta: { partial?: boolean; indexed_at?: string | null } | null | undefined,
  nowMs = Date.now(),
  maxAgeMs = 7 * 24 * 60 * 60 * 1000,
): boolean {
  if (!meta) return false;
  if (meta.partial) return true;
  if (!meta.indexed_at) return false;
  const t = Date.parse(meta.indexed_at);
  if (!Number.isFinite(t)) return false;
  return nowMs - t > maxAgeMs;
}
