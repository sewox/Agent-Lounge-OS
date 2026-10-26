"use client";

import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { DeadSymbolsVaultLink } from "@/components/dead-symbols-panel";
import { formatDisplayPath } from "@/lib/experience";
import { formatSymbolFileLine } from "@/lib/dead-symbols";
import type { DeadSymbol, SemanticMapSelection } from "@/lib/lounge";
import { shortSymbolName } from "@/lib/lounge";
import {
  computeTokenRowHeight,
  DEAD_SYMBOL_ROW_HEIGHT_REM,
  readRootFontSizePx,
  subscribeRootFontSize,
} from "@/lib/row-height";
import {
  resolveVaultDisplayName,
  windowSlice,
  type VaultProjectRow,
} from "@/lib/vault-projects";

function useDeadRowHeightPx(): number {
  const rootPx = useSyncExternalStore(
    subscribeRootFontSize,
    () => readRootFontSizePx(),
    () => 16,
  );
  return computeTokenRowHeight(rootPx, DEAD_SYMBOL_ROW_HEIGHT_REM);
}

function deadKindLabel(
  kind: string | null | undefined,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  const normalized = (kind || "dead").toLowerCase();
  if (normalized === "unused" || normalized === "broken" || normalized === "dead") {
    return t(`deadKind.${normalized}`);
  }
  return kind || t("deadKind.dead");
}

/**
 * Vault map detail column: virtualized dead list for the selected node.
 * Empty copy is natural-size (KB-UI-1a B1) — only when nothing is selected
 * or the workspace has no projects. No stretched cards / frontend dead counts.
 */
export function VaultDeadSymbolsPane({
  selected,
  symbols,
  projects,
  selectedPath,
}: {
  selected: SemanticMapSelection | null;
  symbols: DeadSymbol[];
  projects: VaultProjectRow[];
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

  const win = windowSlice(symbols, scrollTop, viewportHeight, rowHeight, 6);
  const visible = symbols.slice(win.start, win.end);

  const selectedMeta = useMemo(() => {
    if (!selected) return null;
    const name = selected.kind === "project" ? selected.name : selected.project;
    return projects.find((row) => row.name === name) ?? null;
  }, [projects, selected]);

  return (
    <div
      data-qa="vault-dead-pane"
      className="flex h-full min-h-0 w-full flex-col overflow-hidden p-2.5 font-body"
    >
      <div className="mb-2 flex shrink-0 items-center justify-between font-mono text-meta font-semibold tracking-wider text-outline">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface">
          {t("layout.detailHeading")}
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
            : null}
        </span>
      </div>

      {!selected ? (
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
      ) : symbols.length === 0 ? (
        <div data-qa="vault-detail-summary" className="space-y-2 overflow-auto">
          <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2.5 py-3">
            <p
              className="font-mono text-body font-medium text-on-surface"
              lang="en"
              title={selectedMeta?.canonicalId || selected.name}
            >
              {selectedMeta ? resolveVaultDisplayName(selectedMeta) : selected.name}
            </p>
            {selectedPath ? (
              <p
                className="mt-1 break-all font-mono text-meta text-on-surface-variant"
                lang="en"
                title={formatDisplayPath(selectedPath)}
              >
                {formatDisplayPath(selectedPath)}
              </p>
            ) : null}
            {selectedMeta ? (
              <p className="mt-2 font-mono text-meta text-outline">
                {t("pageCountMeta", {
                  pages: selectedMeta.pageCount,
                  experiences: selectedMeta.experienceCount,
                })}
                {` · ${t("astNodes", { count: selectedMeta.nodeCount })} · ${t("edges", {
                  count: selectedMeta.edgeCount,
                })}`}
              </p>
            ) : null}
            <p className="mt-3 font-body text-body text-on-surface-variant">
              {t("noDeadForNode", {
                name: selectedMeta ? resolveVaultDisplayName(selectedMeta) : selected.name,
              })}
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
                {visible.map((symbol) => {
                  const shortName = shortSymbolName(symbol.name) || symbol.name;
                  const fileLine = formatSymbolFileLine(symbol);
                  const kindText = deadKindLabel(symbol.kind, t);
                  const detail = symbol.detail?.trim() || "";
                  const location = symbol.file
                    ? fileLine
                    : detail || kindText;
                  const secondaryTitle = [location, detail].filter(Boolean).join(" · ");
                  return (
                    <div
                      role="listitem"
                      key={`${symbol.name}:${symbol.file ?? ""}:${symbol.line ?? ""}:${symbol.kind}`}
                      data-qa="vault-dead-row"
                      className="flex items-start justify-between gap-2 border-b border-error/20 bg-error/5 px-1.5 py-1"
                      style={{ height: rowHeight }}
                    >
                      <div className="min-w-0">
                        <div
                          className="truncate font-medium text-error"
                          lang="en"
                          title={symbol.name}
                          aria-label={symbol.name}
                        >
                          {shortName}
                        </div>
                        <div
                          className="truncate text-meta text-on-surface-variant"
                          lang="en"
                          title={secondaryTitle}
                        >
                          {location}
                          {detail && symbol.file ? (
                            <span className="text-outline">{` · ${detail}`}</span>
                          ) : null}
                        </div>
                      </div>
                      <span
                        data-qa="vault-dead-kind"
                        className="shrink-0 rounded border border-error/40 px-1 text-meta text-error"
                      >
                        {kindText}
                      </span>
                    </div>
                  );
                })}
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
