"use client";

import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import {
  modeShowsDetailInline,
  modeShowsInspectorInline,
  resolvePanelMode,
  type PanelMode,
} from "./panel-mode";

export type PanelGridProps = {
  list: ReactNode;
  detail?: ReactNode;
  inspector?: ReactNode;
  /** Initial portrait splitter ratio (list height / total). Default 0.42 */
  defaultSplitRatio?: number;
  className?: string;
  /** Called when container-driven mode changes (tests / telemetry). */
  onModeChange?: (mode: PanelMode) => void;
};

const SPLIT_MIN = 0.2;
const SPLIT_MAX = 0.8;

export function PanelGrid({
  list,
  detail,
  inspector,
  defaultSplitRatio = 0.42,
  className = "",
  onModeChange,
}: PanelGridProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [box, setBox] = useState({ width: 0, height: 0 });
  const [drawer, setDrawer] = useState<"detail" | "inspector" | null>(null);
  const [splitRatio, setSplitRatio] = useState(defaultSplitRatio);
  const dragging = useRef(false);
  const titleId = useId();

  useEffect(() => {
    const el = rootRef.current;
    if (!el || typeof ResizeObserver === "undefined") {
      return;
    }
    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const { width, height } = entry.contentRect;
      setBox({ width, height });
    });
    ro.observe(el);
    const rect = el.getBoundingClientRect();
    setBox({ width: rect.width, height: rect.height });
    return () => ro.disconnect();
  }, []);

  const mode = resolvePanelMode(box.width || 1, box.height || 1);

  useEffect(() => {
    onModeChange?.(mode);
  }, [mode, onModeChange]);

  const detailInline = modeShowsDetailInline(mode) && detail != null;
  const inspectorInline = modeShowsInspectorInline(mode) && inspector != null;
  const detailInDrawer = detail != null && !detailInline;
  const inspectorInDrawer = inspector != null && !inspectorInline;
  // Ignore stale drawer requests once the slot becomes inline.
  const openDrawer =
    drawer === "detail" && detailInDrawer
      ? "detail"
      : drawer === "inspector" && inspectorInDrawer
        ? "inspector"
        : null;

  const onSplitterPointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    dragging.current = true;
    event.currentTarget.setPointerCapture(event.pointerId);
  }, []);

  const onSplitterPointerMove = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragging.current || !rootRef.current) return;
    const rect = rootRef.current.getBoundingClientRect();
    if (rect.height <= 0) return;
    const next = (event.clientY - rect.top) / rect.height;
    setSplitRatio(Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, next)));
  }, []);

  const onSplitterPointerUp = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    dragging.current = false;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }, []);

  const portraitStyle: CSSProperties | undefined =
    mode === "portrait-stack" && detailInline
      ? {
          gridTemplateRows: `${splitRatio}fr auto ${1 - splitRatio}fr`,
        }
      : undefined;

  return (
    <div
      ref={rootRef}
      data-qa="panel-grid"
      data-panel-mode={mode}
      data-panel-width={Math.round(box.width)}
      className={`relative flex h-full min-h-0 w-full min-w-0 flex-1 flex-col ${className}`}
    >
      {(detailInDrawer || inspectorInDrawer) && (
        <div
          data-qa="panel-grid-drawer-triggers"
          className="mb-2 flex shrink-0 flex-wrap gap-2"
        >
          {detailInDrawer ? (
            <button
              type="button"
              data-qa="open-detail-drawer"
              className="min-h-8 rounded border border-outline-variant bg-surface-container px-2.5 py-1 font-body text-body text-on-surface hover:bg-surface-container-high"
              onClick={() => setDrawer("detail")}
            >
              Open detail
            </button>
          ) : null}
          {inspectorInDrawer ? (
            <button
              type="button"
              data-qa="open-inspector-drawer"
              className="min-h-8 rounded border border-outline-variant bg-surface-container px-2.5 py-1 font-body text-body text-on-surface hover:bg-surface-container-high"
              onClick={() => setDrawer("inspector")}
            >
              Open inspector
            </button>
          ) : null}
        </div>
      )}

      <div
        data-qa="panel-grid-body"
        className={
          mode === "three"
            ? "grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_minmax(0,0.9fr)] gap-3"
            : mode === "two"
              ? "grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)] gap-3"
              : mode === "portrait-stack" && detailInline
                ? "grid min-h-0 flex-1 gap-0"
                : "flex min-h-0 flex-1 flex-col"
        }
        style={portraitStyle}
      >
        <section
          data-qa="panel"
          data-panel-slot="list"
          className="flex min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
        >
          {list}
        </section>

        {mode === "portrait-stack" && detailInline ? (
          <div
            role="separator"
            aria-orientation="horizontal"
            aria-valuemin={Math.round(SPLIT_MIN * 100)}
            aria-valuemax={Math.round(SPLIT_MAX * 100)}
            aria-valuenow={Math.round(splitRatio * 100)}
            data-qa="panel-grid-splitter"
            tabIndex={0}
            className="z-[1] h-2 cursor-row-resize bg-outline-variant/80 hover:bg-primary/60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary"
            onPointerDown={onSplitterPointerDown}
            onPointerMove={onSplitterPointerMove}
            onPointerUp={onSplitterPointerUp}
            onPointerCancel={onSplitterPointerUp}
            onKeyDown={(event) => {
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setSplitRatio((r) => Math.max(SPLIT_MIN, r - 0.04));
              } else if (event.key === "ArrowDown") {
                event.preventDefault();
                setSplitRatio((r) => Math.min(SPLIT_MAX, r + 0.04));
              }
            }}
          />
        ) : null}

        {detailInline ? (
          <section
            data-qa="panel"
            data-panel-slot="detail"
            className="flex min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
          >
            {detail}
          </section>
        ) : null}

        {inspectorInline ? (
          <section
            data-qa="panel"
            data-panel-slot="inspector"
            className="flex min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
          >
            {inspector}
          </section>
        ) : null}
      </div>

      {openDrawer ? (
        <div
          data-qa="panel-grid-drawer"
          className="absolute inset-0 z-20 flex justify-end bg-surface/60"
          role="presentation"
          onClick={() => setDrawer(null)}
        >
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby={titleId}
            data-qa={`panel-grid-drawer-${openDrawer}`}
            className="flex h-full w-full max-w-lg flex-col border-l border-outline-variant bg-surface-container shadow-xl"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex shrink-0 items-center justify-between border-b border-outline-variant px-3 py-2">
              <h2 id={titleId} className="font-body text-panel font-semibold tracking-label uppercase">
                {openDrawer === "detail" ? "Detail" : "Inspector"}
              </h2>
              <button
                type="button"
                data-qa="close-panel-drawer"
                className="min-h-8 rounded border border-outline-variant px-2 py-1 font-body text-body"
                onClick={() => setDrawer(null)}
              >
                Close
              </button>
            </div>
            <div className="min-h-0 flex-1 overflow-auto p-3">
              {openDrawer === "detail" ? detail : inspector}
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
