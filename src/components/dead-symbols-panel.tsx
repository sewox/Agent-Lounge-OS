"use client";

import Link from "next/link";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLounge } from "@/components/lounge-provider";
import { IndexEmptyState } from "@/components/index-empty-state";
import { Icon } from "@/components/icons";
import { StatsStrip } from "@/components/design-system";
import {
  deadSymbolKey,
  formatSymbolFileLine,
  formatSymbolFullPathLine,
  sortDeadSymbols,
  type DeadSymbolSort,
} from "@/lib/dead-symbols";
import {
  countDeadByCategory,
  deadSymbolReason,
  filterDeadSymbolsUi,
  isScanStale,
  type DeadCategoryFilter,
  type DeadConfidenceFilter,
} from "@/lib/health-agents";
import { hasIndexedWorkspace, isTauri, type DeadSymbol } from "@/lib/lounge";
import { deadSymbolStrings as s } from "@/lib/strings/dead-symbols";

const ROW_HEIGHT_PX = 52;
const OVERSCAN = 8;

type DeadSymbolsPanelProps = {
  projectFilter?: string | null;
  mode?: "dead" | "ignored";
};

type ScanMeta = {
  partial: boolean;
  indexed_at: string | null;
};

export function DeadSymbolsPanel({
  projectFilter = null,
  mode = "dead",
}: DeadSymbolsPanelProps) {
  const { t } = useTranslation("health");
  const {
    deadSymbols,
    ignoredSymbols,
    projects,
    lastIndex,
    semanticMap,
    ignoreDeadSymbol,
    unignoreDeadSymbol,
    openDeadSymbolInEditor,
    copyDeadSymbolPath,
    fixDeadSymbolWithAgent,
    deadSymbolNotice,
    clearDeadSymbolNotice,
  } = useLounge();

  const source = mode === "ignored" ? ignoredSymbols : deadSymbols;
  const indexed = hasIndexedWorkspace({ lastIndex, projects, semanticMap });
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<DeadCategoryFilter>("all");
  const [confidence, setConfidence] = useState<DeadConfidenceFilter>("all");
  const [sort, setSort] = useState<DeadSymbolSort>("name");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(320);
  const [scanMeta, setScanMeta] = useState<ScanMeta>({ partial: false, indexed_at: null });
  const listRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function loadScanMeta() {
      // Prefer QA fixture override (e2e partial scan), then Tauri list_health_scans.
      const qa = (typeof window !== "undefined"
        ? (window as Window & { __QA_HEALTH_SCAN_META__?: ScanMeta }).__QA_HEALTH_SCAN_META__
        : undefined) ?? null;
      if (qa) {
        if (!cancelled) setScanMeta(qa);
        return;
      }
      if (!isTauri()) {
        if (!cancelled) setScanMeta({ partial: false, indexed_at: null });
        return;
      }
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const jobs = await invoke<
          {
            result?: { partial?: boolean; indexed_at?: string | null; project_id?: string } | null;
          }[]
        >("list_health_scans");
        const match =
          (projectFilter
            ? jobs.find((j) => j.result?.project_id === projectFilter)
            : undefined) ?? jobs[0];
        const result = match?.result;
        if (!cancelled) {
          setScanMeta({
            partial: Boolean(result?.partial),
            indexed_at: result?.indexed_at ?? null,
          });
        }
      } catch {
        if (!cancelled) setScanMeta({ partial: false, indexed_at: null });
      }
    }
    void loadScanMeta();
    return () => {
      cancelled = true;
    };
  }, [projectFilter, indexed, source.length]);

  const scoped = useMemo(() => {
    if (!projectFilter) return source;
    return source.filter((row) => !row.project_id || row.project_id === projectFilter);
  }, [source, projectFilter]);

  const counts = useMemo(() => countDeadByCategory(scoped), [scoped]);

  const filtered = useMemo(
    () =>
      sortDeadSymbols(
        filterDeadSymbolsUi(scoped, {
          query,
          category,
          confidence,
          projectId: null,
          partial: scanMeta.partial,
        }),
        sort,
      ),
    [scoped, query, category, confidence, sort, scanMeta.partial],
  );

  const selected = useMemo(
    () => filtered.find((row) => deadSymbolKey(row) === selectedKey) ?? filtered[0] ?? null,
    [filtered, selectedKey],
  );

  const stale = isScanStale(scanMeta);

  useEffect(() => {
    const el = listRef.current;
    if (!el) {
      return;
    }
    const measure = () => setViewportH(el.clientHeight || 320);
    measure();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : null;
    ro?.observe(el);
    return () => ro?.disconnect();
  }, [indexed, filtered.length]);

  const windowStart = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT_PX) - OVERSCAN);
  const windowCount = Math.ceil(viewportH / ROW_HEIGHT_PX) + OVERSCAN * 2;
  const windowed = filtered.slice(windowStart, windowStart + windowCount);
  const topPad = windowStart * ROW_HEIGHT_PX;
  const bottomPad = Math.max(0, (filtered.length - windowStart - windowed.length) * ROW_HEIGHT_PX);

  const statsItems = [
    { id: "all", label: t("dead.statsTotal"), value: counts.total },
    { id: "unused", label: t("dead.statsUnused"), value: counts.unused },
    { id: "broken", label: t("dead.statsBroken"), value: counts.broken },
    {
      id: "partial",
      label: t("dead.statsPartial"),
      value: scanMeta.partial ? t("dead.partialYes") : t("dead.partialNo"),
    },
  ];

  if (!indexed) {
    return (
      <section
        data-qa="panel"
        data-qa-dead-symbols={mode}
        className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
      >
        <PanelHeader mode={mode} total={0} projectFilter={projectFilter} />
        <IndexEmptyState detail="No data found." className="min-h-0 flex-1" />
      </section>
    );
  }

  return (
    <section
      data-qa="panel"
      data-qa-dead-symbols={mode}
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
    >
      <PanelHeader mode={mode} total={filtered.length} projectFilter={projectFilter} />
      {mode === "dead" ? (
        <div className="shrink-0 space-y-2 border-b border-outline-variant/40 px-2.5 py-2">
          <StatsStrip
            items={statsItems}
            activeId={category === "all" ? "all" : category}
            onFilterChange={(id) => {
              if (id === "all" || id === "unused" || id === "broken" || id === "partial") {
                setCategory(id);
              }
            }}
          />
          {stale ? (
            <div
              role="status"
              data-qa="dead-stale-warning"
              className="rounded border border-tertiary/50 bg-surface-container-highest px-2.5 py-2 font-body text-body text-tertiary"
            >
              {t("dead.staleWarning")}
            </div>
          ) : null}
          <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label={t("dead.categoryChipAll")}>
            {(
              [
                ["all", t("dead.categoryChipAll")],
                ["unused", t("kindUnused")],
                ["broken", t("kindBroken")],
              ] as const
            ).map(([id, label]) => (
              <button
                key={id}
                type="button"
                data-qa="dead-category-chip"
                data-chip={id}
                aria-pressed={category === id}
                onClick={() => setCategory(id)}
                className={`min-h-8 rounded border px-2 py-1 font-body text-meta focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary ${
                  category === id
                    ? "border-primary bg-primary-container/25 text-primary"
                    : "border-outline-variant text-on-surface-variant hover:bg-surface-container-high"
                }`}
              >
                {label}
              </button>
            ))}
            <select
              value={confidence}
              onChange={(event) =>
                setConfidence(event.target.value as DeadConfidenceFilter)
              }
              className="min-h-8 rounded border border-outline-variant bg-surface-container px-2 font-body text-meta"
              aria-label={t("dead.confidenceAll")}
              data-qa="dead-confidence-filter"
            >
              <option value="all">{t("dead.confidenceAll")}</option>
              <option value="Kritik">{t("dead.confidenceKritik")}</option>
              <option value="Potansiyel">{t("dead.confidencePotansiyel")}</option>
            </select>
          </div>
        </div>
      ) : null}
      {deadSymbolNotice ? (
        <div
          role="status"
          className="mx-2.5 mt-2 shrink-0 rounded border border-primary/40 bg-primary-container/20 px-2.5 py-1.5 font-body text-meta text-on-surface"
        >
          <div className="flex items-center justify-between gap-2">
            <span>{deadSymbolNotice}</span>
            <button
              type="button"
              onClick={clearDeadSymbolNotice}
              className="min-h-8 rounded px-1.5 font-body text-meta text-primary hover:underline"
            >
              OK
            </button>
          </div>
        </div>
      ) : null}
      <div className="flex min-h-0 flex-1 flex-col gap-2 p-2.5 lg:flex-row">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded border border-outline-variant/50 bg-surface-container-low/50">
          <Toolbar
            query={query}
            sort={sort}
            onQuery={setQuery}
            onSort={setSort}
          />
          <div
            ref={listRef}
            data-qa="dead-symbol-list"
            className="min-h-0 flex-1 overflow-auto font-mono text-body"
            onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          >
            {filtered.length === 0 ? (
              <div
                className="flex min-h-[55vh] flex-col justify-between gap-3 px-3 py-6 text-center"
                data-qa="dead-symbol-empty"
              >
                <div className="space-y-3">
                  <p className="font-body text-body text-on-surface-variant">
                    {mode === "ignored" ? s.noIgnored : s.noSymbols}
                  </p>
                  <p className="font-body text-body text-on-surface-variant">
                    {mode === "ignored"
                      ? t("dead.reasonFallback")
                      : t("dead.staleWarning")}
                  </p>
                  <p className="font-body text-meta text-outline">{s.selectSymbol}</p>
                  <p className="font-body text-meta text-outline">{s.searchPlaceholder}</p>
                </div>
                <div className="space-y-2">
                  <p className="font-body text-meta text-outline">
                    {t("dead.statsTotal")} · {t("dead.statsUnused")} · {t("dead.statsBroken")}
                  </p>
                  <p className="font-body text-meta text-outline">
                    {t("dead.confidenceKritik")} / {t("dead.confidencePotansiyel")}
                  </p>
                  <p className="font-body text-meta text-outline">{s.reindex}</p>
                  <p className="font-body text-meta text-outline">{s.fileLine}</p>
                  <p className="font-body text-meta text-outline">{s.detail}</p>
                  <p className="font-body text-meta text-outline">{s.lastRef}</p>
                </div>
              </div>
            ) : (
              <div style={{ paddingTop: topPad, paddingBottom: bottomPad }}>
                {windowed.map((symbol) => {
                  const key = deadSymbolKey(symbol);
                  const active = selected && deadSymbolKey(selected) === key;
                  return (
                    <button
                      key={key}
                      type="button"
                      data-qa="dead-symbol-row"
                      onClick={() => setSelectedKey(key)}
                      style={{ minHeight: ROW_HEIGHT_PX }}
                      className={`flex w-full items-start justify-between gap-2 border-b border-outline-variant/30 px-2.5 py-2 text-left transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-primary ${
                        active
                          ? "bg-primary-container/30"
                          : "hover:bg-surface-container-high/60"
                      }`}
                    >
                      <div className="min-w-0">
                        <div className="truncate font-medium text-error">{symbol.name}</div>
                        <div className="truncate text-meta text-on-surface-variant">
                          {formatSymbolFileLine(symbol)}
                        </div>
                      </div>
                      <span className="shrink-0 rounded border border-error/40 px-1 text-meta uppercase text-error">
                        {symbol.kind || "dead"}
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
        <SymbolDetail
          symbol={selected}
          mode={mode}
          onOpen={() => selected && void openDeadSymbolInEditor(selected)}
          onCopy={() => selected && void copyDeadSymbolPath(selected)}
          onIgnore={() => selected && void ignoreDeadSymbol(selected)}
          onUnignore={() => selected && void unignoreDeadSymbol(selected)}
          onFix={() => selected && void fixDeadSymbolWithAgent(selected)}
          reason={
            selected
              ? deadSymbolReason(selected, {
                  unused: t("dead.reasonUnused"),
                  broken: t("dead.reasonBroken"),
                  kritik: t("dead.reasonKritik"),
                  potansiyel: t("dead.reasonPotansiyel"),
                  fallback: t("dead.reasonFallback"),
                })
              : ""
          }
          reasonLabel={t("dead.reasonLabel")}
        />
      </div>
    </section>
  );
}

function PanelHeader({
  mode,
  total,
  projectFilter,
}: {
  mode: "dead" | "ignored";
  total: number;
  projectFilter?: string | null;
}) {
  return (
    <div className="flex shrink-0 items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
      <div className="flex items-center gap-2">
        <span className="text-error">
          <Icon name="rule" />
        </span>
        <h3 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {mode === "ignored" ? s.ignoreListTitle : s.panelTitle}
        </h3>
        {projectFilter ? (
          <span className="rounded border border-outline-variant px-1.5 py-0.5 font-mono text-meta text-outline">
            {projectFilter}
          </span>
        ) : null}
      </div>
      <span
        data-qa="dead-symbol-total"
        data-qa-total={total}
        className="font-mono text-meta text-outline"
      >
        {s.total(total)}
      </span>
    </div>
  );
}

function Toolbar({
  query,
  sort,
  onQuery,
  onSort,
}: {
  query: string;
  sort: DeadSymbolSort;
  onQuery: (value: string) => void;
  onSort: (value: DeadSymbolSort) => void;
}) {
  return (
    <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-outline-variant/40 p-2">
      <input
        type="search"
        value={query}
        onChange={(event) => onQuery(event.target.value)}
        placeholder={s.searchPlaceholder}
        className="min-h-8 min-w-[10rem] flex-1 rounded border border-outline-variant bg-surface-container px-2 font-body text-body text-on-surface"
        aria-label={s.searchPlaceholder}
      />
      <select
        value={sort}
        onChange={(event) => onSort(event.target.value as DeadSymbolSort)}
        className="min-h-8 rounded border border-outline-variant bg-surface-container px-2 font-body text-meta"
        aria-label="Sort"
      >
        <option value="name">{s.sortName}</option>
        <option value="file">{s.sortFile}</option>
        <option value="kind">{s.sortKind}</option>
      </select>
    </div>
  );
}

function SymbolDetail({
  symbol,
  mode,
  onOpen,
  onCopy,
  onIgnore,
  onUnignore,
  onFix,
  reason,
  reasonLabel,
}: {
  symbol: DeadSymbol | null;
  mode: "dead" | "ignored";
  onOpen: () => void;
  onCopy: () => void;
  onIgnore: () => void;
  onUnignore: () => void;
  onFix: () => void;
  reason: string;
  reasonLabel: string;
}) {
  return (
    <div
      data-qa="dead-symbol-detail"
      className="flex min-h-[12rem] min-w-0 flex-1 flex-col overflow-hidden rounded border border-outline-variant/50 bg-surface-container-high/40 p-2.5 lg:min-w-[18rem] lg:max-w-[28rem]"
    >
      {!symbol ? (
        <div className="flex flex-1 items-center justify-center text-center font-body text-body text-on-surface-variant">
          {s.selectSymbol}
        </div>
      ) : (
        <>
          <div className="min-h-0 flex-1 space-y-2 overflow-auto font-body text-body">
            <div>
              <div className="font-mono text-meta uppercase text-outline">{symbol.kind}</div>
              <div className="break-words font-mono text-panel font-semibold text-error">
                {symbol.name}
              </div>
            </div>
            <DetailRow label={s.project} value={symbol.project_id ?? "—"} />
            <DetailRow label={s.fileLine} value={formatSymbolFullPathLine(symbol)} />
            <DetailRow label={s.detail} value={symbol.detail ?? "—"} />
            <DetailRow label={reasonLabel} value={reason || "—"} />
            <DetailRow
              label={s.lastRef}
              value={symbol.last_ref?.trim() ? symbol.last_ref : s.noLastRef}
            />
          </div>
          <div className="mt-2 flex shrink-0 flex-wrap gap-1.5 border-t border-outline-variant/40 pt-2">
            {mode === "dead" ? (
              <>
                <ActionButton label={s.openInEditor} onClick={onOpen} />
                <ActionButton label={s.copyPath} onClick={onCopy} />
                <ActionButton label={s.ignore} onClick={onIgnore} tone="warn" />
                <ActionButton label={s.fixWithAgent} onClick={onFix} tone="primary" />
              </>
            ) : (
              <>
                <ActionButton label={s.openInEditor} onClick={onOpen} />
                <ActionButton label={s.copyPath} onClick={onCopy} />
                <ActionButton label={s.unignore} onClick={onUnignore} tone="primary" />
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function DetailRow({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="font-mono text-meta uppercase tracking-wider text-outline">{label}</div>
      <div className="break-all font-mono text-body text-on-surface">{value}</div>
    </div>
  );
}

function ActionButton({
  label,
  onClick,
  tone = "neutral",
}: {
  label: string;
  onClick: () => void;
  tone?: "neutral" | "warn" | "primary";
}) {
  const toneClass =
    tone === "warn"
      ? "border-error/40 text-error hover:bg-error/10"
      : tone === "primary"
        ? "border-primary/40 text-primary hover:bg-primary-container/30"
        : "border-outline-variant text-on-surface hover:bg-surface-container-high";
  return (
    <button
      type="button"
      onClick={onClick}
      className={`min-h-8 rounded border px-2 py-1 font-body text-meta font-medium ${toneClass}`}
    >
      {label}
    </button>
  );
}

export function DeadSymbolsVaultLink({
  count,
  project,
}: {
  count: number;
  project?: string | null;
}) {
  if (count <= 0) {
    return null;
  }
  const href = project
    ? `/health?tab=dead&project=${encodeURIComponent(project)}`
    : "/health?tab=dead";
  return (
    <Link
      href={href}
      className="font-body text-meta font-medium text-primary hover:underline"
    >
      {s.viewAll(count)}
    </Link>
  );
}
