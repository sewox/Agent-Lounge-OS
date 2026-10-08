"use client";

import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLounge } from "@/components/lounge-provider";
import {
  autoPortInfoPorts,
  isEnableGraphUiDisabled,
  shouldBlockEnableOnConflict,
  type GraphUiButtonStatus,
} from "@/lib/graph-ui-button-state";
import { isTauri } from "@/lib/lounge";

export type GraphUiStatus = GraphUiButtonStatus & {
  binary_found: boolean;
  cbm_project_name: string | null;
  owned_by_lounge: boolean;
};

type GraphUiButtonProps = {
  /** Aktif Lounge projesinin kök yolu (canonical eşleme için). */
  projectRoot?: string | null;
};

export function GraphUiButton({ projectRoot }: GraphUiButtonProps) {
  const { t } = useTranslation("vault");
  const { projects, selectedProject, semanticMap } = useLounge();
  const [status, setStatus] = useState<GraphUiStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const resolvedRoot = useMemo(() => {
    if (projectRoot?.trim()) {
      return projectRoot.trim();
    }
    if (!selectedProject) {
      return null;
    }
    const fromProjects = projects.find((row) => row.name === selectedProject)?.root_path;
    if (fromProjects) {
      return fromProjects;
    }
    const fromMap = semanticMap.projects.find((row) => row.name === selectedProject)?.repo_path;
    return fromMap || null;
  }, [projectRoot, projects, selectedProject, semanticMap.projects]);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    const load = async () => {
      try {
        const next = await invoke<GraphUiStatus>("get_graph_ui_status", {
          projectRoot: resolvedRoot,
        });
        if (!cancelled) {
          setStatus(next);
        }
      } catch {
        if (!cancelled) {
          setStatus(null);
        }
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [resolvedRoot]);

  useEffect(() => {
    if (!toast) {
      return;
    }
    const timer = window.setTimeout(() => setToast(null), 6_000);
    return () => window.clearTimeout(timer);
  }, [toast]);

  if (!status?.binary_found) {
    return null;
  }

  const remap = autoPortInfoPorts(status);
  const infoNote = remap
    ? t("graphAutoPortInfo", { busy: remap.busy, next: remap.next })
    : status.info_message?.trim() || null;

  return (
    <GraphUiButtonView
      status={status}
      busy={busy}
      toast={toast}
      infoNote={infoNote}
      labels={{
        enable: t("graphEnable"),
        enableWithPort: t("graphEnableWithPort", { port: status.port }),
        open: t("graphOpen"),
        indexFirst: t("graphIndexFirst"),
        busyEllipsis: "…",
      }}
      onEnable={async () => {
        if (busy) {
          return;
        }
        if (shouldBlockEnableOnConflict(status)) {
          setToast(status.conflict_message || t("graphPortBusy", { port: status.port }));
          return;
        }
        const confirmed = await ask(t("graphEnableConfirmBody", { port: status.port }), {
          title: t("graphEnableConfirmTitle"),
          kind: "warning",
          okLabel: t("graphEnableConfirmOk"),
          cancelLabel: t("graphEnableConfirmCancel"),
        });
        if (!confirmed) {
          return;
        }
        setBusy(true);
        try {
          await invoke("enable_graph_ui_cmd", { projectRoot: resolvedRoot });
          const next = await invoke<GraphUiStatus>("get_graph_ui_status", {
            projectRoot: resolvedRoot,
          });
          setStatus(next);
        } catch (err) {
          setToast(err instanceof Error ? err.message : String(err));
          try {
            const next = await invoke<GraphUiStatus>("get_graph_ui_status", {
              projectRoot: resolvedRoot,
            });
            setStatus(next);
          } catch {
            /* probe fail — toast yeterli */
          }
        } finally {
          setBusy(false);
        }
      }}
      onOpen={async () => {
        if (busy) {
          return;
        }
        setBusy(true);
        try {
          await invoke("open_graph_ui", { projectRoot: resolvedRoot });
        } catch (err) {
          setToast(err instanceof Error ? err.message : String(err));
        } finally {
          setBusy(false);
        }
      }}
    />
  );
}

type GraphUiButtonViewLabels = {
  enable: string;
  enableWithPort: string;
  open: string;
  indexFirst: string;
  busyEllipsis: string;
};

export type GraphUiButtonViewProps = {
  status: GraphUiButtonStatus;
  busy: boolean;
  toast?: string | null;
  infoNote?: string | null;
  labels: GraphUiButtonViewLabels;
  onEnable: () => void | Promise<void>;
  onOpen: () => void | Promise<void>;
};

/** Presentational surface — exported for unit tests. */
export function GraphUiButtonView({
  status,
  busy,
  toast,
  infoNote,
  labels,
  onEnable,
  onOpen,
}: GraphUiButtonViewProps) {
  const showEnable = !status.ui_available;
  const showOpen = status.ui_available && status.project_indexed;
  const showIndexedHint = status.ui_available && !status.project_indexed;
  const enableDisabled = isEnableGraphUiDisabled(busy, status);
  const showInfo = Boolean(infoNote?.trim());
  const enableLabel = showInfo ? labels.enableWithPort : labels.enable;
  const conflictTitle =
    status.port_conflict && status.port_mode === "user"
      ? status.conflict_message || undefined
      : undefined;

  return (
    <div className="flex flex-col items-end gap-1" data-qa="graph-ui-button">
      {showEnable ? (
        <button
          type="button"
          data-qa="graph-ui-enable"
          disabled={enableDisabled}
          title={conflictTitle || (showInfo ? infoNote || undefined : undefined)}
          onClick={() => void onEnable()}
          className="rounded border border-outline-variant bg-surface-container-high px-2.5 py-1 font-body text-meta font-semibold text-on-surface hover:bg-surface-bright disabled:cursor-not-allowed disabled:opacity-50"
        >
          {busy ? labels.busyEllipsis : enableLabel}
        </button>
      ) : null}
      {showOpen ? (
        <button
          type="button"
          data-qa="graph-ui-open"
          disabled={busy}
          onClick={() => void onOpen()}
          className="rounded bg-primary-container px-2.5 py-1 font-body text-meta font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
        >
          {busy ? labels.busyEllipsis : labels.open}
        </button>
      ) : null}
      {showIndexedHint ? (
        <button
          type="button"
          disabled
          title={labels.indexFirst}
          className="cursor-not-allowed rounded border border-outline-variant/60 bg-surface-container-high/50 px-2.5 py-1 font-body text-meta text-on-surface-variant opacity-70"
        >
          {labels.open}
        </button>
      ) : null}
      {showInfo ? (
        <p
          className="max-w-[18rem] text-right font-body text-meta text-on-surface-variant"
          role="status"
          data-qa="graph-ui-auto-info"
        >
          {infoNote}
        </p>
      ) : null}
      {toast ? (
        <p className="max-w-[18rem] text-right font-body text-meta text-error" role="status">
          {toast}
        </p>
      ) : null}
    </div>
  );
}
