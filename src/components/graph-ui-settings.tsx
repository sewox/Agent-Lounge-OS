"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@/lib/lounge";

const DEFAULT_PORT = 9749;

type GraphUiLiveStatus = {
  binary_found: boolean;
  ui_available: boolean;
  project_indexed: boolean;
  port: number;
  port_conflict: boolean;
  conflict_message: string | null;
};

export function GraphUiSettings() {
  const { t } = useTranslation("settings");
  const [port, setPort] = useState(DEFAULT_PORT);
  const [draft, setDraft] = useState(String(DEFAULT_PORT));
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [live, setLive] = useState<GraphUiLiveStatus | null>(null);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    const load = async () => {
      try {
        const value = await invoke<number>("get_graph_ui_port");
        if (cancelled) {
          return;
        }
        setPort(value);
        setDraft(String(value));
      } catch {
        if (!cancelled) {
          setPort(DEFAULT_PORT);
          setDraft(String(DEFAULT_PORT));
        }
      }
      try {
        const status = await invoke<GraphUiLiveStatus>("get_graph_ui_status", {
          projectRoot: null,
        });
        if (!cancelled) {
          setLive(status);
        }
      } catch {
        if (!cancelled) {
          setLive(null);
        }
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 15_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const save = async () => {
    if (!isTauri() || saving) {
      return;
    }
    const parsed = Number.parseInt(draft.trim(), 10);
    if (!Number.isFinite(parsed) || parsed < 1 || parsed > 65535) {
      setMessage(t("graphPortInvalid"));
      return;
    }
    setSaving(true);
    setMessage(null);
    try {
      const saved = await invoke<number>("set_graph_ui_port", { port: parsed });
      setPort(saved);
      setDraft(String(saved));
      setMessage(t("graphPortSaved", { port: saved }));
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const stateLabel = !live
    ? t("graphStateUnknown")
    : live.port_conflict
      ? t("graphStateConflict", { port: live.port })
      : live.ui_available
        ? t("graphStateEnabled", { port: live.port })
        : t("graphStateDisabled", { port: live.port });

  return (
    <div data-qa="graph-ui-settings" className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("graphUiTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("graphUiDesc", { port: DEFAULT_PORT })}
        </p>
      </div>
      <div className="space-y-3 p-3">
        <p className="font-body text-body text-on-surface" data-qa="graph-ui-state">
          {stateLabel}
        </p>
        <p className="font-body text-meta text-outline">{t("graphUiLifecycle")}</p>
        <div className="flex flex-wrap items-end gap-2">
          <label className="space-y-1 font-body text-meta text-on-surface-variant">
            {t("graphPort")}
            <input
              type="number"
              min={1}
              max={65535}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              className="block w-28 rounded border border-outline-variant bg-surface-container-low px-2 py-1 font-body text-body text-on-surface"
            />
          </label>
          <button
            type="button"
            disabled={saving || draft === String(port)}
            onClick={() => void save()}
            className="rounded bg-primary-container px-2.5 py-1 font-body text-meta font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
          >
            {saving ? "…" : t("graphPortSave")}
          </button>
        </div>
        {message ? (
          <p className="font-body text-meta text-on-surface-variant" role="status">
            {message}
          </p>
        ) : null}
      </div>
    </div>
  );
}
