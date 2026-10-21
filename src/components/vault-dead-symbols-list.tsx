"use client";

import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { DeadSymbolsVaultLink } from "@/components/dead-symbols-panel";
import { formatDisplayPath } from "@/lib/experience";
import type { DeadSymbol, SemanticMapSelection } from "@/lib/lounge";
import {
  computeTokenRowHeight,
  DEAD_SYMBOL_ROW_HEIGHT_REM,
  readRootFontSizePx,
  subscribeRootFontSize,
} from "@/lib/row-height";
import type { VaultProjectRow } from "@/lib/vault-projects";
import { windowSlice } from "@/lib/vault-projects";

function useDeadRowHeightPx(): number {
  const rootPx = useSyncExternalStore(
    subscribeRootFontSize,
    () => readRootFontSizePx(),
    () => 16,
  );
  return computeTokenRowHeight(rootPx, DEAD_SYMBOL_ROW_HEIGHT_REM);
}

function deadCountForProject(symbols: DeadSymbol[], projectName: string): number {
  const key = projectName.toLowerCase();
  return symbols.filter((symbol) => {
    const pid = (symbol.project_id ?? "").toLowerCase();
    return !pid || pid === key || pid.includes(key);
  }).length;
}

/**
 * Vault map detail column: project overview on landing, virtualized dead list
 * when a node is selected. Empty/loading copy stays natural-size (KB-UI-1a B1).
 */
export function VaultDeadSymbolsPane({
  selected,
  symbols,
  projects,
  allDeadSymbols,
  selectedPath,
}: {
  selected: SemanticMapSelection | null;
  symbols: DeadSymbol[];
  projects: VaultProjectRow[];
  allDeadSymbols: DeadSymbol[];
  selectedPath?: string | null;
}) {
  const { t } = useTranslation("vault");
  const rowHeight = useDeadRowHeightPx();
  const [scrollerEl, setScrollerEl] = useState<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(240);

  useEffect(() => {
    if (!scrollerEl) {
      return;
    }
    const update = () => setViewportHeight(scrollerEl.clientHeight || 240);
    update();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(update) : null;
    ro?.observe(scrollerEl);
    return () => ro?.disconnect();
  }, [scrollerEl]);

  const overviewRows = useMemo(
    () =>
      projects.map((row) => ({
        ...row,
        deadCount: deadCountForProject(allDeadSymbols, row.name),
      })),
    [allDeadSymbols, projects],
  );
  const stretchOverview = overviewRows.length > 0 && overviewRows.length <= 8;

  const win = windowSlice(symbols, scrollTop, viewportHeight, rowHeight, 6);
  const visible = symbols.slice(win.start, win.end);

  const selectedProject = useMemo(() => {
    if (!selected) return null;
    const name = selected.kind === "project" ? selected.name : selected.project;
    return projects.find((row) => row.name === name) ?? null;
  }, [projects, selected]);

  return (
    <div
      data-qa="vault-dead-pane"
      className="flex h-full min-h-0 w-full flex-col overflow-hidden p-2.5 font-body"
    >
      <div className="mb-2 flex shrink-0 items-center justify-between font-mono text-meta font-semibold tracking-wider text-outline uppercase">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {selected ? t("layout.detailHeading") : t("layout.overviewHeading")}
        </h2>
        <span
          className={
            selected && symbols.length > 0 ? "text-error" : "text-outline"
          }
        >
          {selected
            ? symbols.length > 0
              ? t("warnings", { count: symbols.length })
              : t("clean")
            : t("projectCount", { count: overviewRows.length })}
        </span>
      </div>

      {!selected ? (
        overviewRows.length === 0 ? (
          <div
            data-qa="vault-dead-empty"
            className="flex min-h-0 flex-1 items-center justify-center"
          >
            <p
              data-qa="layout-empty"
              className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-3 py-4 text-center font-body text-body text-on-surface-variant"
            >
              {t("layout.selectForDetail")}
            </p>
          </div>
        ) : (
          <div
            role="list"
            aria-label={t("layout.overviewListLabel")}
            data-qa="vault-detail-overview"
            className={`min-h-0 flex-1 overflow-auto font-body text-body ${
              stretchOverview ? "flex flex-col gap-1.5" : "space-y-1.5"
            }`}
          >
            {overviewRows.map((row) => (
              <div
                key={`overview:${row.name}`}
                role="listitem"
                data-qa="vault-overview-row"
                data-project={row.name}
                className={`rounded border border-outline-variant/40 bg-surface-container-high/50 px-2 py-2 ${
                  stretchOverview ? "min-h-0 flex-1" : ""
                }`}
              >
                <span className="block truncate font-mono font-medium text-on-surface" title={row.name}>
                  {row.name}
                </span>
                <span className="mt-0.5 block truncate font-mono text-meta text-outline">
                  {t("pageCountMeta", {
                    pages: row.pageCount,
                    experiences: row.experienceCount,
                  })}
                  {` · ${t("astNodes", { count: row.nodeCount })}`}
                </span>
                <span
                  className={`mt-1 block font-mono text-meta ${
                    row.deadCount > 0 ? "text-error" : "text-outline"
                  }`}
                >
                  {row.deadCount > 0
                    ? t("warnings", { count: row.deadCount })
                    : t("clean")}
                </span>
              </div>
            ))}
          </div>
        )
      ) : symbols.length === 0 ? (
        <div
          data-qa="vault-detail-summary"
          className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto"
        >
          <div className="min-h-0 flex-1 rounded border border-outline-variant/40 bg-surface-container-high/40 px-2.5 py-3">
            <p className="font-mono text-body font-medium text-on-surface">{selected.name}</p>
            {selectedPath ? (
              <p
                className="mt-1 break-all font-mono text-meta text-on-surface-variant"
                title={formatDisplayPath(selectedPath)}
              >
                {formatDisplayPath(selectedPath)}
              </p>
            ) : null}
            {selectedProject ? (
              <p className="mt-2 font-mono text-meta text-outline">
                {t("pageCountMeta", {
                  pages: selectedProject.pageCount,
                  experiences: selectedProject.experienceCount,
                })}
                {` · ${t("astNodes", { count: selectedProject.nodeCount })} · ${t("edges", {
                  count: selectedProject.edgeCount,
                })}`}
              </p>
            ) : null}
            <p className="mt-3 font-body text-body text-on-surface-variant">
              {t("noDeadForNode", { name: selected.name })}
            </p>
          </div>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-1">
          <div
            ref={setScrollerEl}
            role="list"
            aria-label={t("layout.deadListLabel")}
            data-qa="vault-dead-list"
            className="min-h-0 flex-1 overflow-auto font-mono text-meta focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-primary"
            tabIndex={0}
            onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          >
            <div style={{ height: win.totalHeight, position: "relative" }}>
              <div style={{ transform: `translateY(${win.offsetY}px)` }}>
                {visible.map((symbol) => (
                  <div
                    role="listitem"
                    key={`${symbol.name}:${symbol.file ?? ""}:${symbol.line ?? ""}:${symbol.kind}`}
                    data-qa="vault-dead-row"
                    className="flex items-start justify-between gap-2 border-b border-error/20 bg-error/5 px-1.5 py-1"
                    style={{ height: rowHeight }}
                  >
                    <div className="min-w-0">
                      <div className="truncate font-medium text-error" title={symbol.name}>
                        {symbol.name}
                      </div>
                      <div
                        className="truncate text-meta text-on-surface-variant"
                        title={
                          symbol.file
                            ? `${formatDisplayPath(symbol.file)}${
                                symbol.line != null ? `:${symbol.line}` : ""
                              }`
                            : symbol.detail || symbol.kind
                        }
                      >
                        {symbol.file
                          ? `${formatDisplayPath(symbol.file)}${
                              symbol.line != null ? `:${symbol.line}` : ""
                            }`
                          : symbol.detail || symbol.kind}
                      </div>
                    </div>
                    <span className="shrink-0 rounded border border-error/40 px-1 text-meta uppercase text-error">
                      {symbol.kind || "dead"}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          </div>
          <DeadSymbolsVaultLink
            count={symbols.length}
            project={selected.kind === "project" ? selected.name : selected.project}
          />
        </div>
      )}
    </div>
  );
}
