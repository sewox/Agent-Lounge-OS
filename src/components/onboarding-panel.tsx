"use client";

import { invoke } from "@tauri-apps/api/core";
import { useRouter } from "next/navigation";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  MOCK_DISCOVERY,
  isTauri,
  type ConnectedTool,
  type DiscoveredTool,
  type DiscoveryReport,
} from "@/lib/lounge";

const SOURCE_LABEL: Record<string, string> = {
  claude_desktop: "Claude Desktop",
  cursor: "Cursor",
  ollama: "Ollama",
  system: "Sistem",
};

function systemToolsAsDiscovered(report: DiscoveryReport): DiscoveredTool[] {
  if (report.tools.some((tool) => tool.kind === "system")) {
    return report.tools.filter((tool) => tool.kind === "system");
  }
  return report.system_tools.map((tool) => ({
    id: tool.id,
    name: tool.name,
    kind: "system",
    source: "system",
    origin_path: tool.path ?? null,
    command: tool.path ?? tool.name,
    args: [],
    endpoint: null,
    detail: tool.detail ?? null,
    available: tool.available,
  }));
}

export function OnboardingPanel() {
  const router = useRouter();
  const [report, setReport] = useState<DiscoveryReport | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scan = useCallback(async () => {
    setScanning(true);
    setError(null);
    try {
      const next = isTauri()
        ? await invoke<DiscoveryReport>("get_discovery_report")
        : MOCK_DISCOVERY;
      let connected: ConnectedTool[] = [];
      if (isTauri()) {
        try {
          connected = await invoke<ConnectedTool[]>("list_connected_tools");
        } catch {
          connected = [];
        }
      }
      const enabled = new Set(connected.filter((row) => row.enabled).map((row) => row.id));
      const initial =
        enabled.size > 0
          ? enabled
          : new Set(next.tools.filter((tool) => tool.available).map((tool) => tool.id));
      setReport(next);
      setSelected(initial);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setReport(MOCK_DISCOVERY);
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    const id = window.setTimeout(() => {
      void scan();
    }, 0);
    return () => window.clearTimeout(id);
  }, [scan]);

  const models = useMemo(() => report?.models ?? [], [report]);
  const mcps = useMemo(() => report?.mcp_servers ?? [], [report]);
  const systemTools = useMemo(
    () => (report ? systemToolsAsDiscovered(report) : []),
    [report],
  );

  const toggle = (id: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const selectGroup = (tools: DiscoveredTool[], on: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      for (const tool of tools) {
        if (on) {
          next.add(tool.id);
        } else {
          next.delete(tool.id);
        }
      }
      return next;
    });
  };

  const selectAll = () => {
    if (!report) {
      return;
    }
    setSelected(new Set(report.tools.map((tool) => tool.id)));
  };

  const save = async () => {
    if (!report) {
      return;
    }
    setSaving(true);
    setError(null);
    const tools = [
      ...models.filter((tool) => selected.has(tool.id)),
      ...mcps.filter((tool) => selected.has(tool.id)),
      ...systemTools.filter((tool) => selected.has(tool.id) && tool.available),
    ];
    try {
      if (isTauri()) {
        await invoke<ConnectedTool[]>("save_selected_tools", { tools });
      }
      router.replace("/stream");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="mx-auto max-w-3xl space-y-3">
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex items-start justify-between gap-3 border-b border-outline-variant bg-surface-container-low p-3">
          <div>
            <h1 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
              Sistem keşfi
            </h1>
            <p className="mt-1 font-body text-[11px] text-on-surface-variant">
              Claude Desktop, Cursor MCP, Ollama ve PATH (git, gh, docker) taranır. Seçtiklerin
              Lounge’a bağlanır; MCP env değerleri kaydedilmez.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void scan()}
            disabled={scanning}
            className="rounded border border-outline-variant bg-surface-container-high px-2 py-1 font-mono text-[11px] text-on-surface hover:bg-surface-bright disabled:opacity-50"
          >
            {scanning ? "Taranıyor…" : "Yeniden tara"}
          </button>
        </div>

        <div className="grid gap-2 p-3 sm:grid-cols-2 lg:grid-cols-4">
          {(report?.sources ?? []).map((source) => (
            <div
              key={source.id}
              className="rounded border border-outline-variant bg-surface-container-high px-3 py-2"
            >
              <div className="flex items-center justify-between font-mono text-[11px]">
                <span className="font-semibold text-on-surface">
                  {SOURCE_LABEL[source.id] ?? source.id}
                </span>
                <span className={source.available ? "text-secondary" : "text-error"}>
                  {source.available ? "found" : "yok"}
                </span>
              </div>
              <div className="mt-1 truncate font-mono text-[10px] text-outline" title={source.origin_path ?? ""}>
                {source.origin_path ?? "—"}
              </div>
              <div className="mt-0.5 font-body text-[10px] text-on-surface-variant">
                {source.detail ?? ""}
              </div>
            </div>
          ))}
          {scanning && !report ? (
            <div className="font-mono text-[11px] text-on-surface-variant">Kaynaklar taranıyor…</div>
          ) : null}
        </div>
      </div>

      {error ? (
        <div className="rounded border border-error-container bg-error-container/20 px-3 py-2 font-mono text-[11px] text-error-dim">
          {error}
        </div>
      ) : null}

      <ToolGroup
        title="Modeller"
        tools={models}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(models, true)}
      />
      <ToolGroup
        title="MCP araçları"
        tools={mcps}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(mcps, true)}
      />
      <ToolGroup
        title="Sistem araçları"
        tools={systemTools}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(systemTools, true)}
      />

      <div className="flex items-center justify-between rounded-lg border border-outline-variant bg-surface-container px-3 py-2">
        <div className="font-mono text-[11px] text-on-surface-variant">
          {selected.size} seçildi
          <button
            type="button"
            onClick={selectAll}
            className="ml-3 text-primary hover:underline"
          >
            Hepsini seç
          </button>
        </div>
        <button
          type="button"
          onClick={() => void save()}
          disabled={saving || !report}
          className="rounded bg-primary-container px-3 py-1.5 text-xs font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
        >
          {saving ? "Kaydediliyor…" : "Sisteme dahil et"}
        </button>
      </div>
    </section>
  );
}

function ToolGroup({
  title,
  tools,
  selected,
  onToggle,
  onSelectAll,
}: {
  title: string;
  tools: DiscoveredTool[];
  selected: Set<string>;
  onToggle: (id: string) => void;
  onSelectAll: () => void;
}) {
  return (
    <div className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
          {title}
          <span className="ml-2 font-medium text-outline">{tools.length}</span>
        </h2>
        {tools.length > 0 ? (
          <button
            type="button"
            onClick={onSelectAll}
            className="font-mono text-[10px] text-primary hover:underline"
          >
            Grubu seç
          </button>
        ) : null}
      </div>
      {tools.length === 0 ? (
        <div className="px-3 py-4 font-mono text-[11px] text-outline">Bu taramada kayıt yok.</div>
      ) : (
        <ul className="divide-y divide-outline-variant/40">
          {tools.map((tool) => {
            const checked = selected.has(tool.id);
            return (
              <li key={tool.id}>
                <label className="flex cursor-pointer items-start gap-3 px-3 py-2 hover:bg-surface-container-high">
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={() => onToggle(tool.id)}
                    className="mt-0.5 accent-primary"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center justify-between gap-2 font-mono text-[11px]">
                      <span className="font-semibold text-on-surface">{tool.name}</span>
                      <span className="shrink-0 text-outline">
                        {SOURCE_LABEL[tool.source] ?? tool.source}
                      </span>
                    </div>
                    <div className="mt-0.5 truncate font-mono text-[10px] text-on-surface-variant">
                      {tool.command
                        ? `${tool.command} ${tool.args.join(" ")}`.trim()
                        : (tool.endpoint ?? tool.origin_path ?? tool.id)}
                    </div>
                    {tool.detail ? (
                      <div className="mt-0.5 font-body text-[10px] text-outline">{tool.detail}</div>
                    ) : null}
                  </div>
                </label>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
