"use client";

import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLounge } from "@/components/lounge-provider";
import {
  resolveGraphUiStatusMessage,
  shouldBlockEnableOnConflict,
  type GraphUiButtonStatus,
} from "@/lib/graph-ui-button-state";
import { GraphUiButtonView } from "@/lib/graph-ui-button-view";
import { isTauri } from "@/lib/lounge";

export type GraphUiStatus = GraphUiButtonStatus & {
  binary_found: boolean;
  cbm_project_name: string | null;
  owned_by_lounge: boolean;
};

export { GraphUiButtonView } from "@/lib/graph-ui-button-view";

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

  const statusNote = resolveGraphUiStatusMessage(status, (key, params) => t(key, params));
  const infoNote =
    status.port_mode === "auto" && !status.port_conflict ? statusNote : null;

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
          setToast(
            resolveGraphUiStatusMessage(status, (key, params) => t(key, params)) ||
              t("graphPortBusy", { port: status.port }),
          );
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
