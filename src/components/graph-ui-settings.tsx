"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@/lib/lounge";

const DEFAULT_PORT = 18749;
const BAND_START = 18749;
const BAND_END = 18759;

type GraphUiPortMode = "auto" | "user";

type GraphUiLiveStatus = {
  binary_found: boolean;
  ui_available: boolean;
  project_indexed: boolean;
  port: number;
  port_conflict: boolean;
  conflict_message: string | null;
  port_mode: GraphUiPortMode;
  owned_by_lounge: boolean;
};

export function GraphUiSettings() {
  const { t } = useTranslation("settings");
  const [port, setPort] = useState(DEFAULT_PORT);
  const [draft, setDraft] = useState(String(DEFAULT_PORT));
  const [mode, setMode] = useState<GraphUiPortMode>("auto");
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
        const modeValue = await invoke<string>("get_graph_ui_port_mode");
        if (cancelled) {
          return;
        }
        setPort(value);
        setDraft(String(value));
        setMode(modeValue === "user" ? "user" : "auto");
      } catch {
        if (!cancelled) {
          setPort(DEFAULT_PORT);
          setDraft(String(DEFAULT_PORT));
          setMode("auto");
        }
      }
      try {
        const status = await invoke<GraphUiLiveStatus>("get_graph_ui_status", {
          projectRoot: null,
        });
        if (!cancelled) {
          setLive(status);
          if (status.port_mode === "auto" || status.port_mode === "user") {
            setMode(status.port_mode);
          }
          if (status.port > 0) {
            setPort(status.port);
            setDraft(String(status.port));
          }
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

  const savePort = async () => {
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
      setMode("user");
      setMessage(t("graphPortSaved", { port: saved }));
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const saveMode = async (next: GraphUiPortMode) => {
    if (!isTauri() || saving) {
      return;
    }
    setSaving(true);
    setMessage(null);
    try {
      const saved = await invoke<string>("set_graph_ui_port_mode", { mode: next });
      setMode(saved === "user" ? "user" : "auto");
      setMessage(
        next === "auto"
          ? t("graphModeSavedAuto", { start: BAND_START, end: BAND_END })
          : t("graphModeSavedUser", { port }),
      );
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

  const ownershipLabel = !live
    ? t("graphOwnershipUnknown")
    : live.owned_by_lounge
      ? t("graphOwnershipOwned")
      : t("graphOwnershipForeign");

  const modeLabel = mode === "auto" ? t("graphModeAuto") : t("graphModeUser");

  return (
    <div
      data-qa="graph-ui-settings"
      className="flex min-h-[16rem] flex-1 flex-col rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("graphUiTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("graphUiDesc", { start: BAND_START, end: BAND_END })}
        </p>
      </div>
      <div className="flex flex-1 flex-col justify-between gap-4 p-3">
        <div className="grid gap-3 sm:grid-cols-2">
          <div className="space-y-2 rounded border border-outline-variant bg-surface-container-high p-3">
            <p className="font-body text-meta font-semibold uppercase tracking-label text-outline">
              {t("graphStatusHeading")}
            </p>
            <p className="font-body text-body text-on-surface" data-qa="graph-ui-state">
              {stateLabel}
            </p>
            <p className="font-body text-body text-on-surface" data-qa="graph-ui-port-mode">
              {t("graphModeLabel")}: {modeLabel}
            </p>
            <p className="font-body text-body text-on-surface" data-qa="graph-ui-ownership">
              {ownershipLabel}
            </p>
            {live?.conflict_message ? (
              <p className="font-body text-meta text-on-surface-variant" role="status">
                {live.conflict_message}
              </p>
            ) : null}
          </div>
          <div className="space-y-2 rounded border border-outline-variant bg-surface-container-high p-3">
            <p className="font-body text-meta font-semibold uppercase tracking-label text-outline">
              {t("graphConfigHeading")}
            </p>
            <p className="font-body text-meta text-on-surface-variant">{t("graphUiLifecycle")}</p>
            <p className="font-body text-meta text-on-surface-variant">
              {t("graphActivePort", { port: live?.port ?? port })}
            </p>
            <div className="flex flex-wrap gap-2" role="radiogroup" aria-label={t("graphModeLabel")}>
              <button
                type="button"
                role="radio"
                aria-checked={mode === "auto"}
                disabled={saving}
                data-qa="graph-ui-mode-auto"
                onClick={() => void saveMode("auto")}
                className={`min-h-8 rounded border px-3 py-1.5 font-body text-body ${
                  mode === "auto"
                    ? "border-primary bg-primary-container/25 font-semibold text-primary"
                    : "border-outline-variant bg-surface-container-low text-on-surface hover:bg-surface-bright"
                }`}
              >
                {t("graphModeAuto")}
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={mode === "user"}
                disabled={saving}
                data-qa="graph-ui-mode-user"
                onClick={() => void saveMode("user")}
                className={`min-h-8 rounded border px-3 py-1.5 font-body text-body ${
                  mode === "user"
                    ? "border-primary bg-primary-container/25 font-semibold text-primary"
                    : "border-outline-variant bg-surface-container-low text-on-surface hover:bg-surface-bright"
                }`}
              >
                {t("graphModeUser")}
              </button>
            </div>
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
                onClick={() => void savePort()}
                data-qa="graph-ui-port-save"
                className="rounded bg-primary-container px-2.5 py-1 font-body text-meta font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
              >
                {saving ? "…" : t("graphPortSave")}
              </button>
            </div>
          </div>
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
