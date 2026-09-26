"use client";

import { useCallback, useSyncExternalStore, type ReactNode } from "react";
import {
  isPanelCollapsed,
  PANEL_COLLAPSE_KEY,
  readPanelCollapsed,
  writePanelCollapsed,
} from "@/lib/ui-prefs";

type CollapsiblePanelProps = {
  id: string;
  title: string;
  children: ReactNode;
  /** When true, panel starts collapsed if no stored preference. */
  defaultCollapsed?: boolean;
  className?: string;
  bodyClassName?: string;
  /** Shown in the header trailing slot (e.g. link). */
  trailer?: ReactNode;
};

const listeners = new Set<() => void>();

function emitCollapseChange() {
  for (const listener of listeners) {
    listener();
  }
}

function subscribeCollapse(listener: () => void) {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key === PANEL_COLLAPSE_KEY || event.key === null) {
      listener();
    }
  };
  if (typeof window !== "undefined") {
    window.addEventListener("storage", onStorage);
  }
  return () => {
    listeners.delete(listener);
    if (typeof window !== "undefined") {
      window.removeEventListener("storage", onStorage);
    }
  };
}

function getCollapseMap() {
  return readPanelCollapsed(typeof window !== "undefined" ? window.localStorage : null);
}

export function CollapsiblePanel({
  id,
  title,
  children,
  defaultCollapsed = false,
  className = "",
  bodyClassName = "",
  trailer,
}: CollapsiblePanelProps) {
  const collapsed = useSyncExternalStore(
    subscribeCollapse,
    () => isPanelCollapsed(id, getCollapseMap(), defaultCollapsed),
    () => defaultCollapsed,
  );

  const toggle = useCallback(() => {
    if (typeof window === "undefined") {
      return;
    }
    const map = readPanelCollapsed(window.localStorage);
    map[id] = !isPanelCollapsed(id, map, defaultCollapsed);
    writePanelCollapsed(map, window.localStorage);
    emitCollapseChange();
  }, [id, defaultCollapsed]);

  return (
    <section
      className={`flex min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container ${className}`}
      data-qa="panel"
      data-panel-id={id}
      data-collapsed={collapsed ? "true" : "false"}
    >
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low px-2.5 py-2">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={!collapsed}
          aria-controls={`panel-body-${id}`}
          className="flex min-w-0 flex-1 items-center gap-2 text-left font-body text-panel font-semibold text-on-surface"
        >
          <span
            className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded border border-outline-variant bg-surface-container-high font-mono text-meta text-on-surface-variant"
            aria-hidden
          >
            {collapsed ? "+" : "−"}
          </span>
          <span className="truncate tracking-label uppercase">{title}</span>
        </button>
        {trailer ? <div className="shrink-0">{trailer}</div> : null}
      </div>
      {!collapsed ? (
        <div id={`panel-body-${id}`} className={`min-h-0 min-w-0 flex-1 ${bodyClassName}`}>
          {children}
        </div>
      ) : null}
    </section>
  );
}
