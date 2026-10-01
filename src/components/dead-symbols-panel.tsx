"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { useLounge } from "@/components/lounge-provider";
import { IndexEmptyState } from "@/components/index-empty-state";
import { Icon } from "@/components/icons";
import {
  deadSymbolKey,
  filterDeadSymbols,
  formatSymbolFileLine,
  formatSymbolFullPathLine,
  sortDeadSymbols,
  type DeadSymbolSort,
} from "@/lib/dead-symbols";
import { hasIndexedWorkspace, type DeadSymbol } from "@/lib/lounge";
import { deadSymbolStrings as s } from "@/lib/strings/dead-symbols";

type DeadSymbolsPanelProps = {
  projectFilter?: string | null;
  mode?: "dead" | "ignored";
};

export function DeadSymbolsPanel({
  projectFilter = null,
  mode = "dead",
}: DeadSymbolsPanelProps) {
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
  const [kind, setKind] = useState<"all" | "unused" | "broken">("all");
  const [sort, setSort] = useState<DeadSymbolSort>("name");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  const filtered = useMemo(
    () => sortDeadSymbols(filterDeadSymbols(source, query, kind, projectFilter), sort),
    [source, query, kind, projectFilter, sort],
  );

  const selected = useMemo(
    () => filtered.find((row) => deadSymbolKey(row) === selectedKey) ?? filtered[0] ?? null,
    [filtered, selectedKey],
  );

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
            kind={kind}
            sort={sort}
            onQuery={setQuery}
            onKind={setKind}
            onSort={setSort}
          />
          <div
            data-qa="dead-symbol-list"
            className="min-h-0 flex-1 overflow-auto font-mono text-body"
          >
            {filtered.length === 0 ? (
              <div className="px-3 py-6 text-center font-body text-body text-on-surface-variant">
                {mode === "ignored" ? s.noIgnored : s.noSymbols}
              </div>
            ) : (
              filtered.map((symbol) => {
                const key = deadSymbolKey(symbol);
                const active = selected && deadSymbolKey(selected) === key;
                return (
                  <button
                    key={key}
                    type="button"
                    data-qa="dead-symbol-row"
                    onClick={() => setSelectedKey(key)}
                    className={`flex w-full items-start justify-between gap-2 border-b border-outline-variant/30 px-2.5 py-2 text-left transition-colors ${
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
              })
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
      <span className="font-mono text-meta text-outline">{s.total(total)}</span>
    </div>
  );
}

function Toolbar({
  query,
  kind,
  sort,
  onQuery,
  onKind,
  onSort,
}: {
  query: string;
  kind: "all" | "unused" | "broken";
  sort: DeadSymbolSort;
  onQuery: (value: string) => void;
  onKind: (value: "all" | "unused" | "broken") => void;
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
        value={kind}
        onChange={(event) => onKind(event.target.value as "all" | "unused" | "broken")}
        className="min-h-8 rounded border border-outline-variant bg-surface-container px-2 font-body text-meta"
        aria-label="Filter kind"
      >
        <option value="all">{s.kindAll}</option>
        <option value="unused">{s.kindUnused}</option>
        <option value="broken">{s.kindBroken}</option>
      </select>
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
}: {
  symbol: DeadSymbol | null;
  mode: "dead" | "ignored";
  onOpen: () => void;
  onCopy: () => void;
  onIgnore: () => void;
  onUnignore: () => void;
  onFix: () => void;
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
