"use client";

import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { useLounge } from "@/components/lounge-provider";
import { isTauri } from "@/lib/lounge";

export type GraphUiStatus = {
  binary_found: boolean;
  ui_available: boolean;
  project_indexed: boolean;
  cbm_project_name: string | null;
  port: number;
  port_conflict: boolean;
  conflict_message: string | null;
};

type GraphUiButtonProps = {
  /** Aktif Lounge projesinin kök yolu (canonical eşleme için). */
  projectRoot?: string | null;
};

export function GraphUiButton({ projectRoot }: GraphUiButtonProps) {
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

  const showEnable = !status.ui_available;
  const showOpen = status.ui_available && status.project_indexed;
  const showIndexedHint = status.ui_available && !status.project_indexed;

  const onEnable = async () => {
    if (busy) {
      return;
    }
    if (status.port_conflict) {
      setToast(status.conflict_message || `Port ${status.port} meşgul`);
      return;
    }
    const confirmed = await ask(
      `Bu işlem codebase-memory-mcp Graph UI'yi kalıcı olarak etkinleştirir (--ui=true) ve port ${status.port} üzerinde çalıştırır.\n\nAyar TÜM codebase-memory oturumları için geçerlidir (Claude, Cursor vb.). Lounge kapanınca yalnızca Lounge'un başlattığı süreç durdurulur; kalıcı ayar geri alınmaz.\n\nDevam edilsin mi?`,
      {
        title: "Graph UI'yi etkinleştir",
        kind: "warning",
        okLabel: "Etkinleştir",
        cancelLabel: "İptal",
      },
    );
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
  };

  const onOpen = async () => {
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
  };

  return (
    <div className="flex flex-col items-end gap-1">
      {showEnable ? (
        <button
          type="button"
          disabled={busy || status.port_conflict}
          title={
            status.port_conflict
              ? status.conflict_message || `Port ${status.port} meşgul`
              : `codebase-memory-mcp Graph UI · port ${status.port}`
          }
          onClick={() => void onEnable()}
          className="rounded border border-outline-variant bg-surface-container-high px-2.5 py-1 font-body text-meta font-semibold text-on-surface hover:bg-surface-bright disabled:cursor-not-allowed disabled:opacity-50"
        >
          {busy ? "…" : "Enable Graph UI"}
        </button>
      ) : null}
      {showOpen ? (
        <button
          type="button"
          disabled={busy}
          onClick={() => void onOpen()}
          className="rounded bg-primary-container px-2.5 py-1 font-body text-meta font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
        >
          {busy ? "…" : "Open 3D Graph"}
        </button>
      ) : null}
      {showIndexedHint ? (
        <button
          type="button"
          disabled
          title="Index workspace first"
          className="cursor-not-allowed rounded border border-outline-variant/60 bg-surface-container-high/50 px-2.5 py-1 font-body text-meta text-on-surface-variant opacity-70"
        >
          Open 3D Graph
        </button>
      ) : null}
      {toast ? (
        <p className="max-w-[18rem] text-right font-body text-meta text-error" role="status">
          {toast}
        </p>
      ) : null}
    </div>
  );
}
