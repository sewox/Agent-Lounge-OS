/** Vault project grouping, filters, and page-list helpers (Bilgi Kasası). */

import type {
  LoungeExperience,
  ProjectSummary,
  SemanticMap,
  SemanticProject,
} from "@/lib/lounge";

export const VAULT_FILTERS_KEY = "al-os-vault-project-filters";

/** Server page window size for drill-down offset pagination. */
export const VAULT_PAGE_WINDOW = 200;

export type VaultSourceType = "indexed" | "discovered" | "imported" | "all";

export type VaultReviewStatus = "all" | "reviewed" | "unreviewed" | "has_experiences";

export type VaultRecentWindow = "all" | "7d" | "30d";

export type VaultProjectFilters = {
  nameQuery: string;
  minPageCount: number;
  recent: VaultRecentWindow;
  reviewStatus: VaultReviewStatus;
  sourceType: VaultSourceType;
};

/** Stable defaults — never return a fresh object from getServerSnapshot. */
export const DEFAULT_VAULT_FILTERS: VaultProjectFilters = {
  nameQuery: "",
  minPageCount: 0,
  recent: "all",
  reviewStatus: "all",
  sourceType: "all",
};

const filterListeners = new Set<() => void>();
let cachedFilterJson: string | null = null;
let cachedFilters: VaultProjectFilters = DEFAULT_VAULT_FILTERS;

function emitVaultFilterChange() {
  for (const listener of filterListeners) {
    listener();
  }
}

export function subscribeVaultFilters(listener: () => void): () => void {
  filterListeners.add(listener);
  if (typeof window !== "undefined") {
    const onStorage = (event: StorageEvent) => {
      if (event.key === VAULT_FILTERS_KEY || event.key === null) {
        listener();
      }
    };
    window.addEventListener("storage", onStorage);
    return () => {
      filterListeners.delete(listener);
      window.removeEventListener("storage", onStorage);
    };
  }
  return () => {
    filterListeners.delete(listener);
  };
}

export function getVaultFiltersSnapshot(): VaultProjectFilters {
  let raw: string | null = null;
  try {
    raw = typeof window !== "undefined" ? window.localStorage.getItem(VAULT_FILTERS_KEY) : null;
  } catch {
    raw = null;
  }
  if (raw === cachedFilterJson) {
    return cachedFilters;
  }
  cachedFilterJson = raw;
  if (!raw) {
    cachedFilters = DEFAULT_VAULT_FILTERS;
    return cachedFilters;
  }
  try {
    cachedFilters = parseVaultFilters(JSON.parse(raw) as unknown);
  } catch {
    cachedFilters = DEFAULT_VAULT_FILTERS;
  }
  return cachedFilters;
}

export function getServerVaultFiltersSnapshot(): VaultProjectFilters {
  return DEFAULT_VAULT_FILTERS;
}

export type VaultProjectRow = {
  name: string;
  repoPath: string | null;
  pageCount: number;
  nodeCount: number;
  edgeCount: number;
  experienceCount: number;
  unreviewedCount: number;
  lastUpdated: string | null;
  sourceType: Exclude<VaultSourceType, "all">;
};

export type ProjectPageRow = {
  path: string;
  title: string;
  snippet: string;
  symbolCount: number;
  lastUpdated: string | null;
};

export type PageSortKey = "path" | "-path" | "title" | "-title" | "updated" | "symbols";

export function parseVaultFilters(raw: unknown): VaultProjectFilters {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return { ...DEFAULT_VAULT_FILTERS };
  }
  const row = raw as Record<string, unknown>;
  const nameQuery = typeof row.nameQuery === "string" ? row.nameQuery : "";
  const minPageCount =
    typeof row.minPageCount === "number" && Number.isFinite(row.minPageCount)
      ? Math.max(0, Math.floor(row.minPageCount))
      : 0;
  const recent =
    row.recent === "7d" || row.recent === "30d" || row.recent === "all"
      ? row.recent
      : "all";
  const reviewStatus =
    row.reviewStatus === "reviewed" ||
    row.reviewStatus === "unreviewed" ||
    row.reviewStatus === "has_experiences" ||
    row.reviewStatus === "all"
      ? row.reviewStatus
      : "all";
  const sourceType =
    row.sourceType === "indexed" ||
    row.sourceType === "discovered" ||
    row.sourceType === "imported" ||
    row.sourceType === "all"
      ? row.sourceType
      : "all";
  return { nameQuery, minPageCount, recent, reviewStatus, sourceType };
}

export function readVaultFilters(
  storage?: Pick<Storage, "getItem"> | null,
): VaultProjectFilters {
  try {
    const raw = storage?.getItem(VAULT_FILTERS_KEY);
    if (!raw) {
      return { ...DEFAULT_VAULT_FILTERS };
    }
    return parseVaultFilters(JSON.parse(raw) as unknown);
  } catch {
    return { ...DEFAULT_VAULT_FILTERS };
  }
}

export function writeVaultFilters(
  filters: VaultProjectFilters,
  storage?: Pick<Storage, "setItem"> | null,
): void {
  try {
    const json = JSON.stringify(filters);
    storage?.setItem(VAULT_FILTERS_KEY, json);
    cachedFilterJson = json;
    cachedFilters = filters;
    emitVaultFilterChange();
  } catch {
    /* ignore quota / private mode */
  }
}

export function resetVaultFilters(
  storage?: Pick<Storage, "removeItem" | "setItem"> | null,
): VaultProjectFilters {
  try {
    storage?.removeItem?.(VAULT_FILTERS_KEY);
  } catch {
    /* ignore */
  }
  writeVaultFilters(DEFAULT_VAULT_FILTERS, storage);
  return DEFAULT_VAULT_FILTERS;
}

function pathBasename(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const parts = normalized.split("/").filter(Boolean);
  return parts[parts.length - 1] || path;
}

function resolveSourceType(input: {
  pageCount: number;
  nodeCount: number;
  experienceCount: number;
}): Exclude<VaultSourceType, "all"> {
  if (input.pageCount > 0 || input.nodeCount > 0) {
    return "indexed";
  }
  if (input.experienceCount > 0) {
    return "imported";
  }
  return "discovered";
}

function experienceStats(
  experiences: LoungeExperience[],
  projectName: string,
): { count: number; unreviewed: number; lastUpdated: string | null } {
  const needle = projectName.toLowerCase();
  let count = 0;
  let unreviewed = 0;
  let lastUpdated: string | null = null;
  for (const row of experiences) {
    if ((row.project_id || "").toLowerCase() !== needle) {
      continue;
    }
    if ((row.status ?? "active") !== "active") {
      continue;
    }
    count += 1;
    if (row.reviewed === false) {
      unreviewed += 1;
    }
    const stamp = row.updated_at || row.created_at || null;
    if (stamp && (!lastUpdated || stamp > lastUpdated)) {
      lastUpdated = stamp;
    }
  }
  return { count, unreviewed, lastUpdated };
}

/** Build one row per project from map / summaries / experiences (client fallback). */
export function buildVaultProjectRows(input: {
  semanticMap: SemanticMap;
  projects: ProjectSummary[];
  experiences: LoungeExperience[];
  aggregates?: VaultProjectRow[] | null;
}): VaultProjectRow[] {
  if (input.aggregates && input.aggregates.length > 0) {
    return input.aggregates.map((row) => ({ ...row }));
  }

  const byName = new Map<string, VaultProjectRow>();

  const upsert = (partial: Partial<VaultProjectRow> & { name: string }) => {
    const key = partial.name;
    const prev = byName.get(key);
    const pageCount = partial.pageCount ?? prev?.pageCount ?? 0;
    const nodeCount = partial.nodeCount ?? prev?.nodeCount ?? 0;
    const edgeCount = partial.edgeCount ?? prev?.edgeCount ?? 0;
    const experienceCount = partial.experienceCount ?? prev?.experienceCount ?? 0;
    const unreviewedCount = partial.unreviewedCount ?? prev?.unreviewedCount ?? 0;
    const lastUpdated = pickNewer(partial.lastUpdated ?? null, prev?.lastUpdated ?? null);
    const repoPath = partial.repoPath ?? prev?.repoPath ?? null;
    const sourceType =
      partial.sourceType ??
      prev?.sourceType ??
      resolveSourceType({ pageCount, nodeCount, experienceCount });
    byName.set(key, {
      name: key,
      repoPath,
      pageCount,
      nodeCount,
      edgeCount,
      experienceCount,
      unreviewedCount,
      lastUpdated,
      sourceType,
    });
  };

  for (const project of input.semanticMap.projects) {
    const pages = uniquePageCount(project);
    const stats = experienceStats(input.experiences, project.name);
    upsert({
      name: project.name || "unnamed",
      repoPath: project.repo_path || null,
      pageCount: pages || project.files || 0,
      nodeCount: project.node_count || project.nodes.length,
      edgeCount: project.edge_count || project.references.length,
      experienceCount: stats.count,
      unreviewedCount: stats.unreviewed,
      lastUpdated: stats.lastUpdated,
      sourceType: resolveSourceType({
        pageCount: pages || project.files || 0,
        nodeCount: project.node_count || project.nodes.length,
        experienceCount: stats.count,
      }),
    });
  }

  for (const project of input.projects) {
    const stats = experienceStats(input.experiences, project.name);
    upsert({
      name: project.name || "unnamed",
      repoPath: project.root_path ?? null,
      pageCount: project.files ?? 0,
      nodeCount: project.nodes,
      edgeCount: project.edges,
      experienceCount: stats.count,
      unreviewedCount: stats.unreviewed,
      lastUpdated: stats.lastUpdated,
    });
  }

  // Experience-only projects (no index yet).
  for (const exp of input.experiences) {
    const name = (exp.project_id || "").trim();
    if (!name || byName.has(name)) {
      continue;
    }
    const stats = experienceStats(input.experiences, name);
    upsert({
      name,
      repoPath: null,
      pageCount: 0,
      nodeCount: 0,
      edgeCount: 0,
      experienceCount: stats.count,
      unreviewedCount: stats.unreviewed,
      lastUpdated: stats.lastUpdated,
      sourceType: "imported",
    });
  }

  return [...byName.values()].sort((a, b) =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
  );
}

function pickNewer(a: string | null, b: string | null): string | null {
  if (!a) return b;
  if (!b) return a;
  return a >= b ? a : b;
}

/** Distinct kind=node files — aligned with list_project_pages / aggregate page_count. */
function uniquePageCount(project: SemanticProject): number {
  const files = new Set<string>();
  for (const node of project.nodes) {
    if (node.file?.trim()) {
      files.add(node.file.trim());
    }
  }
  return files.size || project.files || 0;
}

function recentCutoff(window: VaultRecentWindow, nowMs = Date.now()): number | null {
  if (window === "all") {
    return null;
  }
  const days = window === "7d" ? 7 : 30;
  return nowMs - days * 24 * 60 * 60 * 1000;
}

/** Apply session filters to project rows. */
export function filterVaultProjects(
  rows: VaultProjectRow[],
  filters: VaultProjectFilters,
  nowMs = Date.now(),
): VaultProjectRow[] {
  const nameNeedle = filters.nameQuery.trim().toLowerCase();
  const cutoff = recentCutoff(filters.recent, nowMs);
  return rows.filter((row) => {
    if (nameNeedle) {
      const hay = `${row.name} ${row.repoPath ?? ""}`.toLowerCase();
      if (!hay.includes(nameNeedle)) {
        return false;
      }
    }
    if (row.pageCount < filters.minPageCount) {
      return false;
    }
    if (cutoff != null) {
      if (!row.lastUpdated) {
        return false;
      }
      const ts = Date.parse(row.lastUpdated);
      if (!Number.isFinite(ts) || ts < cutoff) {
        return false;
      }
    }
    if (filters.sourceType !== "all" && row.sourceType !== filters.sourceType) {
      return false;
    }
    switch (filters.reviewStatus) {
      case "has_experiences":
        return row.experienceCount > 0;
      case "unreviewed":
        return row.unreviewedCount > 0;
      case "reviewed":
        return row.experienceCount > 0 && row.unreviewedCount === 0;
      default:
        return true;
    }
  });
}

/** Derive page rows from a semantic project (client / mock fallback). */
export function pagesFromSemanticProject(project: SemanticProject | null | undefined): ProjectPageRow[] {
  if (!project) {
    return [];
  }
  const byPath = new Map<string, ProjectPageRow>();
  for (const node of project.nodes) {
    const path = node.file?.trim();
    if (!path) {
      continue;
    }
    const prev = byPath.get(path);
    if (!prev) {
      byPath.set(path, {
        path,
        title: pathBasename(path),
        snippet: node.name || node.kind || "",
        symbolCount: 1,
        lastUpdated: null,
      });
    } else {
      prev.symbolCount += 1;
      if (node.name && !prev.snippet.includes(node.name)) {
        prev.snippet = `${prev.snippet}, ${node.name}`.slice(0, 160);
      }
    }
  }
  return [...byPath.values()].sort((a, b) => a.path.localeCompare(b.path));
}

export function filterAndSortPages(
  pages: ProjectPageRow[],
  query: string,
  sort: PageSortKey = "path",
): ProjectPageRow[] {
  const needle = query.trim().toLowerCase();
  let rows = pages;
  if (needle) {
    rows = pages.filter((page) => {
      const hay = `${page.path} ${page.title} ${page.snippet}`.toLowerCase();
      return hay.includes(needle);
    });
  }
  const sorted = rows.slice();
  switch (sort) {
    case "title":
      sorted.sort((a, b) => a.title.localeCompare(b.title));
      break;
    case "-title":
      sorted.sort((a, b) => b.title.localeCompare(a.title));
      break;
    case "updated":
      sorted.sort((a, b) => (b.lastUpdated ?? "").localeCompare(a.lastUpdated ?? "") || a.path.localeCompare(b.path));
      break;
    case "symbols":
      sorted.sort((a, b) => b.symbolCount - a.symbolCount || a.path.localeCompare(b.path));
      break;
    case "-path":
      sorted.sort((a, b) => b.path.localeCompare(a.path));
      break;
    default:
      sorted.sort((a, b) => a.path.localeCompare(b.path));
  }
  return sorted;
}

/** Simple windowed slice for virtualized lists (no extra dependency). */
export function windowSlice<T>(
  items: T[],
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscan = 6,
): { start: number; end: number; offsetY: number; totalHeight: number } {
  const total = items.length;
  const totalHeight = total * rowHeight;
  if (total === 0 || rowHeight <= 0 || viewportHeight <= 0) {
    return { start: 0, end: 0, offsetY: 0, totalHeight };
  }
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const visible = Math.ceil(viewportHeight / rowHeight) + overscan * 2;
  const end = Math.min(total, start + visible);
  return { start, end, offsetY: start * rowHeight, totalHeight };
}

export function debounceMs(): number {
  return 250;
}

/** Map wire aggregate (snake_case) → UI row. */
export function mapVaultAggregate(raw: Record<string, unknown>): VaultProjectRow {
  const name = String(raw.name ?? raw.project_id ?? "unnamed");
  const pageCount = Number(raw.page_count ?? raw.pageCount ?? raw.files ?? 0) || 0;
  const nodeCount = Number(raw.node_count ?? raw.nodeCount ?? raw.nodes ?? 0) || 0;
  const edgeCount = Number(raw.edge_count ?? raw.edgeCount ?? raw.edges ?? 0) || 0;
  const experienceCount = Number(raw.experience_count ?? raw.experienceCount ?? 0) || 0;
  const unreviewedCount = Number(raw.unreviewed_count ?? raw.unreviewedCount ?? 0) || 0;
  const lastUpdated =
    (typeof raw.last_updated === "string" && raw.last_updated) ||
    (typeof raw.lastUpdated === "string" && raw.lastUpdated) ||
    null;
  const repoPath =
    (typeof raw.repo_path === "string" && raw.repo_path) ||
    (typeof raw.repoPath === "string" && raw.repoPath) ||
    (typeof raw.root_path === "string" && raw.root_path) ||
    null;
  const sourceRaw = String(raw.source_type ?? raw.sourceType ?? "");
  const sourceType: Exclude<VaultSourceType, "all"> =
    sourceRaw === "discovered" || sourceRaw === "imported" || sourceRaw === "indexed"
      ? sourceRaw
      : resolveSourceType({ pageCount, nodeCount, experienceCount });
  return {
    name,
    repoPath,
    pageCount,
    nodeCount,
    edgeCount,
    experienceCount,
    unreviewedCount,
    lastUpdated,
    sourceType,
  };
}

export function mapProjectPage(raw: Record<string, unknown>): ProjectPageRow {
  const path = String(raw.path ?? "");
  return {
    path,
    title: String(raw.title ?? pathBasename(path)),
    snippet: String(raw.snippet ?? ""),
    symbolCount: Number(raw.symbol_count ?? raw.symbolCount ?? 0) || 0,
    lastUpdated:
      (typeof raw.last_updated === "string" && raw.last_updated) ||
      (typeof raw.lastUpdated === "string" && raw.lastUpdated) ||
      null,
  };
}
