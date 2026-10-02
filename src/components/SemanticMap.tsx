"use client";

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "@/components/icons";
import { IndexEmptyState } from "@/components/index-empty-state";
import { Pager } from "@/components/ui";
import { formatDisplayPath } from "@/lib/experience";
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
  symbolsFromSemanticFile,
  windowSlice,
  writeVaultFilters,
  type FileSymbolRow,
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
  aggregatesLoading?: boolean;
  aggregatesError?: string | null;
  onRetryAggregates?: () => void;
  /** Optional server page list for the open project. */
  projectPages?: ProjectPageRow[] | null;
  projectPagesTotal?: number | null;
  pagesLoading?: boolean;
  pagesError?: string | null;
  onPageSearch?: (query: string, sort: PageSortKey) => void;
  onLoadMorePages?: () => void;
  onRetryPages?: () => void;
  /** Symbols for the expanded page (list_file_symbols). */
  fileSymbols?: FileSymbolRow[] | null;
  fileSymbolsLoading?: boolean;
  onOpenPage?: (path: string) => void;
  onClosePage?: () => void;
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
  const { t } = useTranslation("vault");
  return (
    <div
      data-qa="vault-project-filters"
      className="mb-2 shrink-0 space-y-1.5 rounded border border-outline-variant/50 bg-surface-container-high/40 p-1.5"
    >
      <div className="flex flex-wrap items-center gap-1.5">
        <label className="sr-only" htmlFor="vault-project-search">
          {t("searchProjects")}
        </label>
        <input
          id="vault-project-search"
          data-qa="vault-filter-name"
          type="search"
          value={filters.nameQuery}
          onChange={(event) => onChange({ ...filters, nameQuery: event.target.value })}
          placeholder={t("searchProjects")}
          className="min-h-8 min-w-0 flex-1 rounded border border-outline-variant bg-surface-container px-2 py-1 font-body text-meta text-on-surface"
        />
        <button
          type="button"
          data-qa="vault-filter-reset"
          onClick={onReset}
          className="min-h-8 shrink-0 rounded border border-outline-variant px-2 py-1 font-body text-meta text-on-surface-variant hover:bg-surface-container-high"
        >
          {t("resetFilters")}
        </button>
      </div>
      <div className="flex flex-wrap items-center gap-1.5 font-body text-meta">
        <label className="flex items-center gap-1 text-on-surface-variant">
          <span>{t("minPages")}</span>
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
          aria-label={t("filters")}
          value={filters.recent}
          onChange={(event) =>
            onChange({ ...filters, recent: event.target.value as VaultRecentWindow })
          }
          className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
        >
          <option value="all">{t("recentAll")}</option>
          <option value="7d">{t("recent7d")}</option>
          <option value="30d">{t("recent30d")}</option>
        </select>
        <select
          data-qa="vault-filter-status"
          aria-label={t("statusAll")}
          value={filters.reviewStatus}
          onChange={(event) =>
            onChange({ ...filters, reviewStatus: event.target.value as VaultReviewStatus })
          }
          className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
        >
          <option value="all">{t("statusAll")}</option>
          <option value="has_experiences">{t("statusHasExperiences")}</option>
          <option value="reviewed">{t("statusReviewed")}</option>
          <option value="unreviewed">{t("statusUnreviewed")}</option>
        </select>
        <select
          data-qa="vault-filter-source"
          aria-label={t("sourceAll")}
          value={filters.sourceType}
          onChange={(event) =>
            onChange({ ...filters, sourceType: event.target.value as VaultSourceType })
          }
          className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 text-on-surface"
        >
          <option value="all">{t("sourceAll")}</option>
          <option value="indexed">{t("sourceIndexed")}</option>
          <option value="discovered">{t("sourceDiscovered")}</option>
          <option value="imported">{t("sourceImported")}</option>
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
  stretch,
}: {
  row: VaultProjectRow;
  active: boolean;
  onSelect: () => void;
  onOpen: () => void;
  stretch?: boolean;
}) {
  const { t } = useTranslation("vault");
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
        stretch ? "min-h-0 flex-1" : ""
      } ${
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
            {t("pageCountMeta", { pages: row.pageCount, experiences: row.experienceCount })}
            {` · ${t("astNodes", { count: row.nodeCount })} · ${t("edges", { count: row.edgeCount })}`}
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
        {t("openProject")}
      </button>
    </div>
  );
}

function VirtualPageList({
  pages,
  query,
  loading,
  onNearEnd,
  activePath,
  onOpenPage,
}: {
  pages: ProjectPageRow[];
  query: string;
  loading?: boolean;
  onNearEnd?: () => void;
  activePath?: string | null;
  onOpenPage?: (path: string) => void;
}) {
  const { t } = useTranslation("vault");
  const [scrollerEl, setScrollerEl] = useState<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(320);

  // Rebind ResizeObserver when the scroller mounts (empty → loaded).
  useEffect(() => {
    if (!scrollerEl) {
      return;
    }
    const update = () => setViewportHeight(scrollerEl.clientHeight || 320);
    update();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(update) : null;
    ro?.observe(scrollerEl);
    return () => ro?.disconnect();
  }, [scrollerEl]);

  const win = windowSlice(pages, scrollTop, viewportHeight, PAGE_ROW_HEIGHT, 8);
  const visible = pages.slice(win.start, win.end);

  if (pages.length === 0 && !loading) {
    return (
      <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center font-body text-body text-on-surface-variant">
        {query.trim() ? t("noPagesMatch") : t("noPages")}
      </div>
    );
  }

  if (pages.length === 0 && loading) {
    return (
      <div
        data-qa="vault-pages-loading"
        className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center font-body text-body text-on-surface-variant"
      >
        {t("pagesLoading")}
      </div>
    );
  }

  return (
    <div
      ref={setScrollerEl}
      data-qa="vault-page-list"
      className="min-h-0 flex-1 overflow-auto"
      onScroll={(event) => {
        const el = event.currentTarget;
        setScrollTop(el.scrollTop);
        if (
          onNearEnd &&
          el.scrollHeight - el.scrollTop - el.clientHeight < PAGE_ROW_HEIGHT * 12
        ) {
          onNearEnd();
        }
      }}
    >
      <div style={{ height: win.totalHeight, position: "relative" }}>
        <div style={{ transform: `translateY(${win.offsetY}px)` }}>
          {visible.map((page) => {
            const active = activePath === page.path;
            return (
              <button
                type="button"
                key={page.path}
                data-qa="vault-page-row"
                data-path={page.path}
                data-active={active ? "true" : "false"}
                onClick={() => onOpenPage?.(page.path)}
                className={`flex w-full items-start justify-between gap-2 border-b border-outline-variant/30 px-1.5 py-1.5 text-left font-body text-body ${
                  active
                    ? "bg-primary-container/20 text-primary"
                    : "text-on-surface hover:bg-surface-container-high/70"
                }`}
                style={{ height: PAGE_ROW_HEIGHT }}
              >
                <span className="min-w-0">
                  <span className="block truncate font-mono font-medium">
                    {page.title}
                  </span>
                  <span className="block truncate font-mono text-meta text-outline">
                    {formatDisplayPath(page.path)}
                    {page.snippet ? ` · ${page.snippet}` : ""}
                  </span>
                </span>
                <span className="shrink-0 font-mono text-meta text-outline">
                  {t("symbols", { count: page.symbolCount })}
                </span>
              </button>
            );
          })}
        </div>
      </div>
      {loading ? (
        <div
          data-qa="vault-pages-loading-more"
          className="px-2 py-2 text-center font-body text-meta text-on-surface-variant"
        >
          {t("pagesLoading")}
        </div>
      ) : null}
    </div>
  );
}

function PageSymbolsPanel({
  filePath,
  symbols,
  loading,
  deadOnly,
  onDeadOnlyChange,
  onClose,
  onSelectSymbol,
  selectedName,
}: {
  filePath: string;
  symbols: FileSymbolRow[];
  loading: boolean;
  deadOnly: boolean;
  onDeadOnlyChange: (next: boolean) => void;
  onClose: () => void;
  onSelectSymbol: (symbol: FileSymbolRow) => void;
  selectedName: string | null;
}) {
  const { t } = useTranslation("vault");
  const visible = deadOnly ? symbols.filter((s) => s.isDead) : symbols;
  const title = filePath.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) || filePath;
  return (
    <div
      data-qa="vault-page-symbols"
      className="mb-2 max-h-48 shrink-0 overflow-auto rounded border border-outline-variant/50 bg-surface-container-high/40 p-1.5"
    >
      <div className="mb-1 flex flex-wrap items-center gap-2 font-body text-meta">
        <span className="min-w-0 flex-1 truncate font-mono text-on-surface">
          {t("pageSymbols", { file: title })}
        </span>
        <label className="flex items-center gap-1 text-on-surface-variant">
          <input
            data-qa="vault-dead-only"
            type="checkbox"
            checked={deadOnly}
            onChange={(event) => onDeadOnlyChange(event.target.checked)}
          />
          <span>{t("deadOnly")}</span>
        </label>
        <button
          type="button"
          data-qa="vault-page-symbols-close"
          onClick={onClose}
          className="min-h-8 rounded border border-outline-variant px-2 py-1 text-on-surface-variant hover:bg-surface-container-high"
        >
          {t("closePageSymbols")}
        </button>
      </div>
      {loading ? (
        <div className="px-1 py-2 text-on-surface-variant">{t("loading")}</div>
      ) : visible.length === 0 ? (
        <div className="px-1 py-2 text-on-surface-variant">
          {deadOnly ? t("noDeadInPage") : t("noSymbolsInPage")}
        </div>
      ) : (
        <ul className="space-y-0.5" role="list">
          {visible.map((symbol) => {
            const active = selectedName === symbol.name;
            return (
              <li key={`${symbol.name}:${symbol.line ?? ""}:${symbol.kind}`}>
                <button
                  type="button"
                  data-qa="vault-file-symbol"
                  data-dead={symbol.isDead ? "true" : "false"}
                  onClick={() => onSelectSymbol(symbol)}
                  className={`flex w-full items-center justify-between gap-2 rounded px-1.5 py-1 text-left font-mono text-meta ${
                    active
                      ? "bg-primary-container/25 text-primary"
                      : "text-on-surface hover:bg-surface-container-high"
                  }`}
                >
                  <span className="min-w-0 truncate">
                    {symbol.name}
                    {symbol.line != null ? `:${symbol.line}` : ""}
                    <span className="text-outline"> · {symbol.kind}</span>
                  </span>
                  {symbol.isDead ? (
                    <span className="shrink-0 rounded border border-error/40 px-1 text-error">
                      dead
                    </span>
                  ) : (
                    <span className="shrink-0 text-outline">{symbol.refCount}</span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
      )}
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
  aggregatesLoading = false,
  aggregatesError = null,
  onRetryAggregates,
  projectPages = null,
  projectPagesTotal = null,
  pagesLoading = false,
  pagesError = null,
  onPageSearch,
  onLoadMorePages,
  onRetryPages,
  fileSymbols = null,
  fileSymbolsLoading = false,
  onOpenPage,
  onClosePage,
}: SemanticMapProps) {
  const { t } = useTranslation("vault");
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
  const [deadOnly, setDeadOnly] = useState(false);
  const expandedPath =
    selected?.kind === "file" || selected?.kind === "node" ? selected.file || null : null;

  useEffect(() => {
    const handle = window.setTimeout(() => setDebouncedQuery(pageQuery), debounceMs());
    return () => window.clearTimeout(handle);
  }, [pageQuery]);

  useEffect(() => {
    if (!openProjectName) {
      return;
    }
    onPageSearch?.(debouncedQuery, sort);
  }, [debouncedQuery, sort, onPageSearch, openProjectName]);

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
  const stretchRows = visibleProjects.length > 0 && visibleProjects.length <= 8;

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
    if (projectPages && onPageSearch) {
      return projectPages;
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
    onClosePage?.();
    onOpenProject(null);
    setPageQuery("");
    setDebouncedQuery("");
    setDeadOnly(false);
  }, [onClosePage, onOpenProject]);

  const clientFileSymbols = useMemo(() => {
    if (!expandedPath) {
      return [] as FileSymbolRow[];
    }
    if (fileSymbols && fileSymbols.length > 0) {
      return fileSymbols;
    }
    return symbolsFromSemanticFile(semanticProject, expandedPath);
  }, [expandedPath, fileSymbols, semanticProject]);

  if (aggregatesLoading && allProjects.length === 0) {
    return (
      <div
        data-qa="vault-projects-loading"
        className="flex h-full min-h-[12rem] w-full flex-1 items-center justify-center font-body text-body text-on-surface-variant"
      >
        {t("projectsLoading")}
      </div>
    );
  }

  if (aggregatesError && allProjects.length === 0) {
    return (
      <div
        data-qa="vault-projects-error"
        className="flex h-full min-h-[12rem] w-full flex-1 flex-col items-center justify-center gap-2 font-body text-body text-error"
      >
        <span>{aggregatesError || t("projectsError")}</span>
        {onRetryAggregates ? (
          <button
            type="button"
            data-qa="vault-projects-retry"
            onClick={onRetryAggregates}
            className="min-h-8 rounded border border-outline-variant px-2 py-1 text-on-surface-variant hover:bg-surface-container-high"
          >
            {t("retry")}
          </button>
        ) : null}
      </div>
    );
  }

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
            ← {t("backToProjects")}
          </button>
          <nav aria-label="breadcrumb" className="flex min-w-0 items-center gap-1 font-mono text-outline">
            <button type="button" className="hover:text-primary" onClick={goBack}>
              {t("breadcrumbProjects")}
            </button>
            <span>/</span>
            <span className="truncate text-on-surface">{openProject.name}</span>
          </nav>
          <span data-qa="vault-page-total" className="ml-auto text-on-surface-variant">
            {t("pages", { count: pageTotal })}
          </span>
        </div>
        <div className="mb-2 flex shrink-0 flex-wrap items-center gap-1.5">
          <label className="sr-only" htmlFor="vault-page-search">
            {t("searchPages")}
          </label>
          <input
            id="vault-page-search"
            data-qa="vault-page-search"
            type="search"
            value={pageQuery}
            onChange={(event) => setPageQuery(event.target.value)}
            placeholder={t("searchPages")}
            className="min-h-8 min-w-0 flex-1 rounded border border-outline-variant bg-surface-container px-2 py-1 font-body text-meta text-on-surface"
          />
          <select
            data-qa="vault-page-sort"
            aria-label={t("sortPath")}
            value={sort}
            onChange={(event) => setSort(event.target.value as PageSortKey)}
            className="min-h-8 rounded border border-outline-variant bg-surface-container px-1.5 py-1 font-body text-meta text-on-surface"
          >
            <option value="path">{t("sortPath")}</option>
            <option value="title">{t("sortTitle")}</option>
            <option value="updated">{t("sortUpdated")}</option>
            <option value="symbols">{t("sortSymbols")}</option>
          </select>
        </div>
        {pagesError ? (
          <div
            data-qa="vault-pages-error"
            className="mb-2 flex shrink-0 flex-wrap items-center gap-2 rounded border border-error/40 bg-error/5 px-2 py-2 font-body text-body text-error"
          >
            <span>{pagesError || t("pagesError")}</span>
            {onRetryPages ? (
              <button
                type="button"
                data-qa="vault-pages-retry"
                onClick={onRetryPages}
                className="min-h-8 rounded border border-outline-variant px-2 py-1 text-on-surface-variant hover:bg-surface-container-high"
              >
                {t("retry")}
              </button>
            ) : null}
          </div>
        ) : null}
        {expandedPath ? (
          <PageSymbolsPanel
            filePath={expandedPath}
            symbols={clientFileSymbols}
            loading={fileSymbolsLoading}
            deadOnly={deadOnly}
            onDeadOnlyChange={setDeadOnly}
            onClose={() => {
              setDeadOnly(false);
              onClosePage?.();
            }}
            selectedName={selected?.kind === "node" ? selected.name : null}
            onSelectSymbol={(symbol) => {
              onSelect({
                id: `node:${symbol.name}:${symbol.file ?? ""}`,
                name: symbol.name,
                kind: "node",
                project: openProjectName,
                file: symbol.file ?? expandedPath,
              });
            }}
          />
        ) : null}
        <VirtualPageList
          pages={clientPages}
          query={debouncedQuery}
          loading={pagesLoading}
          onNearEnd={onLoadMorePages}
          activePath={expandedPath}
          onOpenPage={(path) => {
            setDeadOnly(false);
            onOpenPage?.(path);
            onSelect({
              id: `file:${path}`,
              name: path.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) || path,
              kind: "file",
              project: openProjectName,
              file: path,
            });
          }}
        />
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
        <span>{t("indexedFiles")}</span>
        <span className="text-on-surface-variant">
          {graphTotals.files > 0
            ? `${t("files", { count: graphTotals.files })} · ${t("projectCount", { count: filteredProjects.length })}`
            : t("projectCount", { count: filteredProjects.length })}
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
      {aggregatesError ? (
        <div
          data-qa="vault-projects-error"
          className="mb-2 flex shrink-0 flex-wrap items-center gap-2 rounded border border-error/40 bg-error/5 px-2 py-2 font-body text-meta text-error"
        >
          <span>{aggregatesError || t("projectsError")}</span>
          {onRetryAggregates ? (
            <button
              type="button"
              data-qa="vault-projects-retry"
              onClick={onRetryAggregates}
              className="min-h-8 rounded border border-outline-variant px-2 py-1 text-on-surface-variant hover:bg-surface-container-high"
            >
              {t("retry")}
            </button>
          ) : null}
        </div>
      ) : null}
      <div
        role="list"
        className={`min-h-0 flex-1 overflow-auto font-body text-body ${
          stretchRows ? "flex flex-col gap-1" : "space-y-1"
        }`}
      >
        {visibleProjects.length === 0 ? (
          <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center text-on-surface-variant">
            {t("noProjects")}
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
                stretch={stretchRows}
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
        <div
          data-qa="vault-projects-summary"
          className="mt-auto shrink-0 border-t border-outline-variant/40 pt-2 font-mono text-meta text-outline"
        >
          <span className="font-body">
            {t("files", { count: graphTotals.files })} · {t("astNodes", { count: graphTotals.nodes })} ·{" "}
            {t("edges", { count: graphTotals.edges })}
          </span>
        </div>
      </div>
      <div className="mt-1.5 flex shrink-0 items-center justify-between border-t border-outline-variant/40 pt-1.5 font-body text-meta text-outline">
        <span>
          {selected
            ? t("selected", { name: selected.name })
            : t("projectCount", { count: filteredProjects.length })}
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
