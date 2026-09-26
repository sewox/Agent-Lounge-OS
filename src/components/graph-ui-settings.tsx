"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { isTauri } from "@/lib/lounge";

const DEFAULT_PORT = 9749;

export function GraphUiSettings() {
  const [port, setPort] = useState(DEFAULT_PORT);
  const [draft, setDraft] = useState(String(DEFAULT_PORT));
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

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
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, []);

  const save = async () => {
    if (!isTauri() || saving) {
      return;
    }
    const parsed = Number.parseInt(draft.trim(), 10);
    if (!Number.isFinite(parsed) || parsed < 1 || parsed > 65535) {
      setMessage("Geçerli bir port girin (1–65535).");
      return;
    }
    setSaving(true);
    setMessage(null);
    try {
      const saved = await invoke<number>("set_graph_ui_port", { port: parsed });
      setPort(saved);
      setDraft(String(saved));
      setMessage(`Graph UI port kaydedildi: ${saved}`);
    } catch (err) {
      setMessage(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
          Graph UI Port
        </h2>
        <p className="mt-1 font-body text-xs text-on-surface-variant">
          codebase-memory-mcp 3D Graph UI dinleme portu (varsayılan {DEFAULT_PORT}). Port başka bir
          süreçte doluysa Settings&apos;ten değiştirin.
        </p>
      </div>
      <div className="flex flex-wrap items-end gap-2 p-3">
        <label className="space-y-1 font-mono text-xs text-on-surface-variant">
          Port
          <input
            type="number"
            min={1}
            max={65535}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            className="block w-28 rounded border border-outline-variant bg-surface-container-low px-2 py-1 font-mono text-xs text-on-surface"
          />
        </label>
        <button
          type="button"
          disabled={saving || draft === String(port)}
          onClick={() => void save()}
          className="rounded bg-primary-container px-2.5 py-1 font-mono text-xs font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
        >
          {saving ? "…" : "Kaydet"}
        </button>
        {message ? (
          <p className="w-full font-body text-xs text-on-surface-variant" role="status">
            {message}
          </p>
        ) : null}
      </div>
    </div>
  );
}
