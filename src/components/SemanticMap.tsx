"use client";

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { Icon } from "@/components/icons";
import { IndexEmptyState } from "@/components/index-empty-state";
import { Pager } from "@/components/ui";
import { formatDisplayPath } from "@/lib/experience";
import { vaultStrings as vaultS } from "@/lib/strings/vault";
import {
  pageCount,
  pageSlice,
  type LoungeExperience,
  type ProjectSummary,
  type SemanticMap as SemanticMapData,
  type SemanticMapSelection,
} from "@/lib/lounge";
import {
  buildVaultProjectRows,
  debounceMs,
  filterAndSortPages,
  filterVaultProjects,
  getServerVaultFiltersSnapshot,
  getVaultFiltersSnapshot,
  pagesFromSemanticProject,
  resetVaultFilters,
  subscribeVaultFilters,
  windowSlice,
  writeVaultFilters,
  type PageSortKey,
  type ProjectPageRow,
  type VaultProjectFilters,
  type VaultProjectRow,
  type VaultRecentWindow,
  type VaultReviewStatus,
  type VaultSourceType,
} from "@/lib/vault-projects";

const PROJECT_PAGE = 24;
const PAGE_ROW_HEIGHT = 36;

type GraphTotals = {
  nodes: number;
  edges: number;
  files: number;
};

type SemanticMapProps = {
  semanticMap: SemanticMapData;
  projects: ProjectSummary[];
  experiences: LoungeExperience[];
  graphTotals: GraphTotals;
  selected: SemanticMapSelection | null;
  onSelect: (next: SemanticMapSelection | null) => void;
  /** Shared drill-down project name; null = project list. */
  openProjectName: string | null;
  onOpenProject: (name: string | null) => void;
  /** Optional server aggregates (list_vault_projects). */
  aggregates?: VaultProjectRow[] | null;
  /** Optional server page list for the open project. */
  projectPages?: ProjectPageRow[] | null;
  projectPagesTotal?: number | null;
  onPageSearch?: (query: string, sort: PageSortKey) => void;
};

function formatStamp(iso: string | null): string {
  if (!iso) {
    return "—";
  }
  const d = Date.parse(iso);
  if (!Number.isFinite(d)) {
    return iso.slice(0, 10);
  }
  try {
    return new Date(d).toLocaleDateString("en-CA");
  } catch {
    return iso.slice(0, 10);
  }
}

function ProjectFiltersBar({
  filters,
  onChange,
  onReset,
}: {
  filters: VaultProjectFilters;
  onChange: (next: VaultProjectFilters) => void;
  onReset: () => void;
}) {
  return (
    <div
      data-qa="vault-project-filters"
      className="mb-2 shrink-0 space-y-1.5 rounded border border-outline-variant/50 bg-surface-container-high/40 p-1.5"
    >
      <div className="flex flex-wrap items-center gap-1.5">
        <label className="sr-only" htmlFor="vault-project-search">
          {vaultS.searchProjects}
        </label>
        <input
          id="vault-project-search"
          data-qa="vault-filter-name"
          type="search"
          value={filters.nameQuery}
          onChange={(event) => onChange({ ...filters, nameQuery: event.target.value })}
          placeholder={vaultS.searchProjects}
          className="min-h-8 min-w-0 flex-1 rounded border border-outline-variant bg-surface-container px-2 py-1 font-body text-meta text-on-surface"
        />
        <button
          type="button"
          data-qa="vault-filter-reset"
          onClick={onReset}
          className="min-h-8 shrink-0 rounded border border-outline-variant px-2 py-1 font-body text-meta text-on-surface-variant hover:bg-surface-container-high"
        >
          {vaultS.resetFilters}
        </button>
      </div>
      <div className="flex flex-wrap items-center gap-1.5 font-body text-meta">
        <label className="flex items-center gap-1 text-on-surface-variant">
          <span>{vaultS.minPages}</span>
          <input
            data-qa="vault-filter-min-pages"
            type="number"
            min={0}
            value={filters.minPageCount}
            onChange={(event) =>
              onChange({
                ...filters,
                minPageCount: Math.max(0, Number(event.target.value) || 0),
              })
            }
            className="w-16 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
          />
        </label>
        <select
          data-qa="vault-filter-recent"
          aria-label={vaultS.filters}
          value={filters.recent}
          onChange={(event) =>
            onChange({ ...filters, recent: event.target.value as VaultRecentWindow })
          }
          className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
        >
          <option value="all">{vaultS.recentAll}</option>
          <option value="7d">{vaultS.recent7d}</option>
          <option value="30d">{vaultS.recent30d}</option>
        </select>
        <select
          data-qa="vault-filter-status"
          aria-label={vaultS.statusAll}
          value={filters.reviewStatus}
          onChange={(event) =>
            onChange({ ...filters, reviewStatus: event.target.value as VaultReviewStatus })
          }
          className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
        >
          <option value="all">{vaultS.statusAll}</option>
          <option value="has_experiences">{vaultS.statusHasExperiences}</option>
          <option value="reviewed">{vaultS.statusReviewed}</option>
          <option value="unreviewed">{vaultS.statusUnreviewed}</option>
        </select>
        <select
          data-qa="vault-filter-source"
          aria-label={vaultS.sourceAll}
          value={filters.sourceType}
          onChange={(event) =>
            onChange({ ...filters, sourceType: event.target.value as VaultSourceType })
          }
          className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
        >
          <option value="all">{vaultS.sourceAll}</option>
          <option value="indexed">{vaultS.sourceIndexed}</option>
          <option value="discovered">{vaultS.sourceDiscovered}</option>
          <option value="imported">{vaultS.sourceImported}</option>
        </select>
      </div>
    </div>
  );
}

function ProjectRow({
  row,
  active,
  onSelect,
  onOpen,
}: {
  row: VaultProjectRow;
  active: boolean;
  onSelect: () => void;
  onOpen: () => void;
}) {
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      onOpen();
    }
  };
  return (
    <div
      role="listitem"
      data-qa="vault-project-row"
      data-project={row.name}
      tabIndex={0}
      onClick={onSelect}
      onDoubleClick={(event) => {
        event.preventDefault();
        onOpen();
      }}
      onKeyDown={onKeyDown}
      className={`flex w-full items-center justify-between gap-2 rounded px-1.5 py-1.5 text-left transition-colors outline-none focus-visible:ring-1 focus-visible:ring-primary ${
        active
          ? "bg-primary-container/25 text-primary"
          : "text-on-surface hover:bg-surface-container-high/80"
      }`}
    >
      <span className="flex min-w-0 items-center gap-1.5">
        <Icon name="folder" className="h-3.5 w-3.5 shrink-0" />
        <span className="min-w-0">
          <span className="block truncate font-medium font-mono">{row.name || "unnamed"}</span>
          <span className="block truncate font-mono text-meta text-outline">
            {vaultS.pageCountMeta(row.pageCount, row.experienceCount)}
            {` · ${vaultS.astNodes(row.nodeCount)} · ${vaultS.edges(row.edgeCount)}`}
            {row.lastUpdated ? ` · ${formatStamp(row.lastUpdated)}` : ""}
          </span>
        </span>
      </span>
      <button
        type="button"
        data-qa="vault-project-open"
        onClick={(event) => {
          event.stopPropagation();
          onOpen();
        }}
        className="min-h-8 shrink-0 rounded border border-outline-variant px-2 py-1 font-body text-meta text-on-surface-variant hover:bg-surface-container-high"
      >
        {vaultS.openProject}
      </button>
    </div>
  );
}

function VirtualPageList({
  pages,
  query,
}: {
  pages: ProjectPageRow[];
  query: string;
}) {
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(320);

  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) {
      return;
    }
    const update = () => setViewportHeight(el.clientHeight || 320);
    update();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(update) : null;
    ro?.observe(el);
    return () => ro?.disconnect();
  }, []);

  const win = windowSlice(pages, scrollTop, viewportHeight, PAGE_ROW_HEIGHT, 8);
  const visible = pages.slice(win.start, win.end);

  if (pages.length === 0) {
    return (
      <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center font-body text-body text-on-surface-variant">
        {query.trim() ? vaultS.noPagesMatch : vaultS.noPages}
      </div>
    );
  }

  return (
    <div
      ref={scrollerRef}
      data-qa="vault-page-list"
      className="min-h-0 flex-1 overflow-auto"
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
    >
      <div style={{ height: win.totalHeight, position: "relative" }}>
        <div style={{ transform: `translateY(${win.offsetY}px)` }}>
          {visible.map((page) => (
            <div
              key={page.path}
              data-qa="vault-page-row"
              className="flex items-start justify-between gap-2 border-b border-outline-variant/30 px-1.5 py-1.5 font-body text-body"
              style={{ height: PAGE_ROW_HEIGHT }}
            >
              <span className="min-w-0">
                <span className="block truncate font-mono font-medium text-on-surface">
                  {page.title}
                </span>
                <span className="block truncate font-mono text-meta text-outline">
                  {formatDisplayPath(page.path)}
                  {page.snippet ? ` · ${page.snippet}` : ""}
                </span>
              </span>
              <span className="shrink-0 font-mono text-meta text-outline">
                {vaultS.symbols(page.symbolCount)}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function SemanticMap({
  semanticMap,
  projects,
  experiences,
  graphTotals,
  selected,
  onSelect,
  openProjectName,
  onOpenProject,
  aggregates = null,
  projectPages = null,
  projectPagesTotal = null,
  onPageSearch,
}: SemanticMapProps) {
  const filters = useSyncExternalStore(
    subscribeVaultFilters,
    getVaultFiltersSnapshot,
    getServerVaultFiltersSnapshot,
  );
  const setFilters = useCallback((next: VaultProjectFilters) => {
    writeVaultFilters(next, typeof window !== "undefined" ? window.localStorage : null);
  }, []);
  const [projectPage, setProjectPage] = useState(0);
  const [pageQuery, setPageQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [sort, setSort] = useState<PageSortKey>("path");

  useEffect(() => {
    const handle = window.setTimeout(() => setDebouncedQuery(pageQuery), debounceMs());
    return () => window.clearTimeout(handle);
  }, [pageQuery]);

  useEffect(() => {
    onPageSearch?.(debouncedQuery, sort);
  }, [debouncedQuery, sort, onPageSearch]);

  const allProjects = useMemo(
    () =>
      buildVaultProjectRows({
        semanticMap,
        projects,
        experiences,
        aggregates,
      }),
    [aggregates, experiences, projects, semanticMap],
  );

  const filteredProjects = useMemo(
    () => filterVaultProjects(allProjects, filters),
    [allProjects, filters],
  );

  const pages = pageCount(filteredProjects.length, PROJECT_PAGE);
  const safeProjectPage = Math.min(projectPage, pages - 1);
  const visibleProjects = pageSlice(filteredProjects, safeProjectPage, PROJECT_PAGE);

  const openProject = useMemo(
    () => allProjects.find((row) => row.name === openProjectName) ?? null,
    [allProjects, openProjectName],
  );

  const semanticProject = useMemo(
    () => semanticMap.projects.find((row) => row.name === openProjectName) ?? null,
    [openProjectName, semanticMap.projects],
  );

  const clientPages = useMemo(() => {
    const base =
      projectPages && projectPages.length > 0
        ? projectPages
        : pagesFromSemanticProject(semanticProject);
    // When server already filtered, still allow client sort/search fallback.
    if (projectPages && projectPages.length > 0 && onPageSearch) {
      return base;
    }
    return filterAndSortPages(base, debouncedQuery, sort);
  }, [debouncedQuery, onPageSearch, projectPages, semanticProject, sort]);

  const pageTotal = projectPagesTotal ?? clientPages.length;

  const openNamed = useCallback(
    (name: string) => {
      onOpenProject(name);
      onSelect({
        id: `project:${name}`,
        name,
        kind: "project",
        project: name,
        file: null,
      });
      setPageQuery("");
      setDebouncedQuery("");
    },
    [onOpenProject, onSelect],
  );

  const goBack = useCallback(() => {
    onOpenProject(null);
    setPageQuery("");
    setDebouncedQuery("");
  }, [onOpenProject]);

  if (allProjects.length === 0) {
    return (
      <div className="flex h-full min-h-[12rem] w-full min-w-0 flex-1 flex-col">
        <IndexEmptyState
          detail="No data found."
          className="h-full min-h-[12rem] flex-1 justify-center"
        />
      </div>
    );
  }

  if (openProjectName && openProject) {
    return (
      <div
        data-qa="vault-semantic-map"
        data-view="pages"
        className="flex min-h-0 w-full flex-1 flex-col overflow-hidden"
      >
        <div className="mb-2 flex shrink-0 flex-wrap items-center gap-2 font-body text-meta">
          <button
            type="button"
            data-qa="vault-back-projects"
            onClick={goBack}
            className="min-h-8 rounded border border-outline-variant px-2 py-1 text-on-surface-variant hover:bg-surface-container-high"
          >
            ← {vaultS.backToProjects}
          </button>
          <nav aria-label="breadcrumb" className="flex min-w-0 items-center gap-1 font-mono text-outline">
            <button type="button" className="hover:text-primary" onClick={goBack}>
              {vaultS.breadcrumbProjects}
            </button>
            <span>/</span>
            <span className="truncate text-on-surface">{openProject.name}</span>
          </nav>
          <span className="ml-auto text-on-surface-variant">
            {vaultS.pages(pageTotal)}
          </span>
        </div>
        <div className="mb-2 flex shrink-0 flex-wrap items-center gap-1.5">
          <label className="sr-only" htmlFor="vault-page-search">
            {vaultS.searchPages}
          </label>
          <input
            id="vault-page-search"
            data-qa="vault-page-search"
            type="search"
            value={pageQuery}
            onChange={(event) => setPageQuery(event.target.value)}
            placeholder={vaultS.searchPages}
            className="min-h-8 min-w-0 flex-1 rounded border border-outline-variant bg-surface-container px-2 py-1 font-body text-meta text-on-surface"
          />
          <select
            data-qa="vault-page-sort"
            aria-label={vaultS.sortPath}
            value={sort}
            onChange={(event) => setSort(event.target.value as PageSortKey)}
            className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 font-body text-meta text-on-surface"
          >
            <option value="path">{vaultS.sortPath}</option>
            <option value="title">{vaultS.sortTitle}</option>
            <option value="updated">{vaultS.sortUpdated}</option>
            <option value="symbols">{vaultS.sortSymbols}</option>
          </select>
        </div>
        <VirtualPageList pages={clientPages} query={debouncedQuery} />
      </div>
    );
  }

  return (
    <div
      data-qa="vault-semantic-map"
      data-view="projects"
      className="flex min-h-0 w-full flex-1 flex-col overflow-hidden"
    >
      <div className="mb-2 flex shrink-0 items-center justify-between font-body text-meta font-semibold tracking-label text-outline uppercase">
        <span>{vaultS.indexedFiles}</span>
        <span className="text-on-surface-variant">
          {graphTotals.files > 0
            ? `${vaultS.files(graphTotals.files)} · ${vaultS.projectCount(filteredProjects.length)}`
            : vaultS.projectCount(filteredProjects.length)}
        </span>
      </div>
      <ProjectFiltersBar
        filters={filters}
        onChange={(next) => {
          setFilters(next);
          setProjectPage(0);
        }}
        onReset={() => {
          setFilters(
            resetVaultFilters(typeof window !== "undefined" ? window.localStorage : null),
          );
          setProjectPage(0);
        }}
      />
      <div role="list" className="min-h-0 flex-1 space-y-1 overflow-auto font-body text-body">
        {visibleProjects.length === 0 ? (
          <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center text-on-surface-variant">
            {vaultS.noProjects}
          </div>
        ) : (
          visibleProjects.map((row) => {
            const isActive =
              selected?.kind === "project" &&
              (selected.name === row.name || selected.project === row.name);
            return (
              <ProjectRow
                key={`proj:${row.name}:${row.repoPath ?? ""}`}
                row={row}
                active={Boolean(isActive)}
                onSelect={() =>
                  onSelect(
                    isActive
                      ? null
                      : {
                          id: `project:${row.name}`,
                          name: row.name,
                          kind: "project",
                          project: row.name,
                          file: null,
                        },
                  )
                }
                onOpen={() => openNamed(row.name)}
              />
            );
          })
        )}
      </div>
      <div className="mt-1.5 flex shrink-0 items-center justify-between border-t border-outline-variant/40 pt-1.5 font-body text-meta text-outline">
        <span>
          {selected ? vaultS.selected(selected.name) : vaultS.projectCount(filteredProjects.length)}
        </span>
        <Pager
          page={safeProjectPage}
          pages={pages}
          total={filteredProjects.length}
          onPage={setProjectPage}
        />
      </div>
    </div>
  );
}
