"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useRouter } from "next/navigation";
import { useCallback, useEffect, useMemo, useState } from "react";
import { BrandMark } from "@/components/brand";
import { Icon } from "@/components/icons";
import {
  MOCK_DISCOVERY,
  MOCK_RECOMMENDED_MODELS,
  MODEL_PULL_EVENT,
  LAYA_ENGINE_EVENT,
  formatDiscoveryLocation,
  layaEnginePercentage,
  hfOfferToTool,
  hostLabelsFor,
  humanizeDiscoveryDetail,
  isTauri,
  type ConnectedTool,
  type DeviceProfile,
  type DiscoveredTool,
  type DiscoveryReport,
  type DiscoverySource,
  type HfModelOffer,
  type PullProgress,
  type RecommendedModels,
  type LayaEngineStatus,
} from "@/lib/lounge";
import { Pip } from "@/components/ui";

const SOURCE_LABEL: Record<string, string> = {
  claude_desktop: "Claude Desktop",
  claude_cli: "Claude CLI",
  cursor: "Cursor",
  grok_bot: "Grok Bot",
  antigravity: "Antigravity",
  lmr: "LMR",
  ollama: "Ollama",
  system: "Sistem",
};

const SOURCE_TITLE: Record<string, string> = {
  lmr: "Lounge Model Runner",
  ollama: "Ollama Sunucusu",
};

const INSTALL: Record<string, { label: string; href: string }> = {
  ollama: { label: "Ollama kur", href: "https://ollama.com/download" },
  claude_desktop: { label: "Claude Desktop", href: "https://claude.ai/download" },
  claude_cli: { label: "Claude CLI", href: "https://docs.anthropic.com/en/docs/claude-code" },
  cursor: { label: "Cursor", href: "https://cursor.com/download" },
  grok_bot: { label: "Grok", href: "https://grok.x.ai" },
  antigravity: { label: "Antigravity", href: "https://antigravity.google" },
  git: { label: "Git kur", href: "https://git-scm.com/downloads" },
  gh: { label: "GitHub CLI", href: "https://cli.github.com" },
  docker: { label: "Docker Desktop", href: "https://docs.docker.com/get-docker/" },
};

function systemToolsAsDiscovered(report: DiscoveryReport): DiscoveredTool[] {
  if (report.tools.some((tool) => tool.kind === "system")) {
    return report.tools.filter((tool) => tool.kind === "system");
  }
  const systemTools = report.system_tools ?? [];
  return systemTools.map((tool) => ({
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
    access_mode: "local",
    host_id: null,
  }));
}

function installFor(id: string): { label: string; href: string } | null {
  return INSTALL[id] ?? null;
}

export function OnboardingPanel() {
  const router = useRouter();
  const [report, setReport] = useState<DiscoveryReport | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [hfSelected, setHfSelected] = useState<Set<string>>(new Set());
  const [device, setDevice] = useState<DeviceProfile | null>(null);
  const [offers, setOffers] = useState<HfModelOffer[]>([]);
  const [pulledTools, setPulledTools] = useState<DiscoveredTool[]>([]);
  const [scanning, setScanning] = useState(true);
  const [saving, setSaving] = useState(false);
  const [pulling, setPulling] = useState(false);
  const [progress, setProgress] = useState<PullProgress | null>(null);
  const [layaEngine, setLayaEngine] = useState<LayaEngineStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const scan = useCallback(async () => {
    setScanning(true);
    setError(null);
    try {
      const next = isTauri()
        ? await invoke<DiscoveryReport>("get_discovery_report")
        : await new Promise<DiscoveryReport>((resolve) => {
            window.setTimeout(() => resolve(MOCK_DISCOVERY), 1400);
          });
      let connected: ConnectedTool[] = [];
      if (isTauri()) {
        try {
          connected = await invoke<ConnectedTool[]>("list_connected_tools");
        } catch {
          connected = [];
        }
      }
      const catalog = isTauri()
        ? await invoke<RecommendedModels>("list_recommended_models")
        : MOCK_RECOMMENDED_MODELS;
      const enabled = new Set(
        connected.filter((row) => row.enabled || row.is_active).map((row) => row.id),
      );
      const initial =
        enabled.size > 0
          ? enabled
          : new Set(next.tools.filter((tool) => tool.available).map((tool) => tool.id));
      for (const offer of catalog.offers) {
        if (offer.installed) {
          initial.add(`lmr:${offer.pull_name}`);
        }
      }
      setReport(next);
      setSelected(initial);
      setDevice(catalog.device);
      setOffers(catalog.offers);
      setHfSelected(
        new Set(
          catalog.offers
            .filter((offer) => offer.recommended && !offer.installed)
            .map((offer) => offer.hf_id),
        ),
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setReport(MOCK_DISCOVERY);
      setDevice(MOCK_RECOMMENDED_MODELS.device);
      setOffers(MOCK_RECOMMENDED_MODELS.offers);
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

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    const boot = window.setTimeout(() => {
      void listen<PullProgress>(MODEL_PULL_EVENT, (event) => {
        if (!cancelled) {
          setProgress(event.payload);
        }
      }).then((fn) => {
        if (cancelled) {
          void fn();
          return;
        }
        unlisten = fn;
      });
    }, 0);
    return () => {
      cancelled = true;
      window.clearTimeout(boot);
      if (unlisten) {
        void unlisten();
      }
    };
  }, []);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    const boot = window.setTimeout(() => {
      void invoke<LayaEngineStatus>("get_laya_engine_status")
        .then((status) => {
          if (!cancelled) {
            setLayaEngine(status);
          }
        })
        .catch(() => {
          /* komut henüz yoksa sessiz */
        });
      void invoke<LayaEngineStatus>("ensure_laya_engine").then((status) => {
        if (!cancelled) {
          setLayaEngine(status);
        }
      }).catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      });
      void listen<LayaEngineStatus>(LAYA_ENGINE_EVENT, (event) => {
        if (!cancelled) {
          setLayaEngine(event.payload);
        }
      }).then((fn) => {
        if (cancelled) {
          void fn();
          return;
        }
        unlisten = fn;
      });
    }, 0);
    return () => {
      cancelled = true;
      window.clearTimeout(boot);
      if (unlisten) {
        void unlisten();
      }
    };
  }, []);

  const apps = useMemo(() => {
    if (!report) {
      return [];
    }
    if (report.apps && report.apps.length > 0) {
      return report.apps;
    }
    return report.tools.filter(
      (tool) => tool.access_mode === "subscription" || tool.kind === "app",
    );
  }, [report]);
  const models = useMemo(() => report?.models ?? [], [report]);
  const plugins = useMemo(() => {
    const rows = report?.mcp_servers ?? [];
    if (rows.length > 0) {
      return rows;
    }
    return (
      report?.tools.filter((tool) => tool.kind === "plugin" || tool.kind === "mcp") ?? []
    );
  }, [report]);
  const systemTools = useMemo(
    () => (report ? systemToolsAsDiscovered(report) : []),
    [report],
  );
  const ollamaSource = report?.sources.find((source) => source.id === "ollama");
  const ollamaInstall =
    ollamaSource && !ollamaSource.available ? installFor("ollama") : null;

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
        if (!tool.available) {
          continue;
        }
        if (on) {
          next.add(tool.id);
        } else {
          next.delete(tool.id);
        }
      }
      return next;
    });
  };

  const toggleHf = (hfId: string, disabled: boolean) => {
    if (disabled) {
      return;
    }
    setHfSelected((current) => {
      const next = new Set(current);
      if (next.has(hfId)) {
        next.delete(hfId);
      } else {
        next.add(hfId);
      }
      return next;
    });
  };

  const pullSelected = async () => {
    const pending = offers.filter(
      (offer) => hfSelected.has(offer.hf_id) && !offer.installed && !offer.heavy,
    );
    if (pending.length === 0) {
      return;
    }
    setPulling(true);
    setError(null);
    try {
      for (const offer of pending) {
        const name = isTauri()
          ? await invoke<string>("pull_lmr_model", { hfId: offer.hf_id })
          : offer.pull_name;
        const tool = hfOfferToTool(offer, name);
        setPulledTools((current) => {
          const rest = current.filter((row) => row.id !== tool.id);
          return [...rest, tool];
        });
        setSelected((current) => {
          const next = new Set(current);
          next.add(tool.id);
          return next;
        });
        setOffers((current) =>
          current.map((row) =>
            row.hf_id === offer.hf_id ? { ...row, installed: true, heavy: false, disabled_reason: null } : row,
          ),
        );
        setHfSelected((current) => {
          const next = new Set(current);
          next.delete(offer.hf_id);
          return next;
        });
      }
      if (isTauri()) {
        try {
          const next = await invoke<DiscoveryReport>("get_discovery_report");
          setReport(next);
        } catch {
          /* tarama yenilenmese de çekilen kayıt durur */
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPulling(false);
    }
  };

  const save = async () => {
    if (!report) {
      return;
    }
    setSaving(true);
    setError(null);
    const byId = new Map<string, DiscoveredTool>();
    for (const tool of [
      ...apps,
      ...models,
      ...pulledTools,
      ...plugins.filter((tool) => selected.has(tool.id)),
      ...systemTools.filter((tool) => selected.has(tool.id) && tool.available),
    ]) {
      if (selected.has(tool.id) || pulledTools.some((row) => row.id === tool.id)) {
        byId.set(tool.id, tool);
      }
    }
    const tools = [...byId.values()];
    try {
      if (isTauri()) {
        await invoke<ConnectedTool[]>("save_selected_tools", { tools });
      }
      router.replace("/dashboard");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  if (scanning && !report) {
    return <ScanningSystem />;
  }

  return (
    <section data-qa="panel" className="mx-auto w-full max-w-none space-y-3 px-0">
      <div className="overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex items-start justify-between gap-3 border-b border-outline-variant bg-surface-container-low p-3">
          <div className="min-w-0">
            <h1 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
              İlk açılış · Sistem keşfi
            </h1>
            <p className="mt-1 font-body text-body text-on-surface-variant">
              Yerel AI uygulamaları, plugin’ler, LMR modelleri ve sistem CLI taranır. Seçtiklerin
              Lounge’a bağlanır; MCP env değerleri kaydedilmez.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void scan()}
            disabled={scanning}
            className="shrink-0 rounded border border-outline-variant bg-surface-container-high px-2 py-1 font-mono text-body text-on-surface hover:bg-surface-bright disabled:opacity-50"
          >
            {scanning ? "Scanning…" : "Yeniden tara"}
          </button>
        </div>
        <SourceTable sources={report?.sources ?? []} />
      </div>

      {error ? (
        <div className="rounded border border-error-container bg-error-container/20 px-3 py-2 font-mono text-body text-error-dim">
          {error}
        </div>
      ) : null}

      <LayaEngineBlock status={layaEngine} />

      <HfCatalogBlock
        device={device}
        offers={offers}
        selected={hfSelected}
        pulling={pulling}
        progress={progress}
        onToggle={toggleHf}
        onPull={() => void pullSelected()}
      />

      <ToolGroup
        title="Yerel AI uygulamaları"
        emptyHint="Claude Desktop, Claude CLI, Cursor, Antigravity veya Grok Bot bulunamadı."
        tools={apps}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(apps, true)}
      />
      <ToolGroup
        title="Plugin’ler"
        emptyHint="Host uygulamalarda MCP plugin’i yok."
        tools={plugins}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(plugins, true)}
      />
      <ToolGroup
        title="Yerel modeller"
        emptyHint="LMR veya Ollama Sunucusu üzerinde yüklü model yok."
        missing={ollamaInstall}
        tools={models}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(models, true)}
      />
      <ToolGroup
        title="Sistem CLI"
        emptyHint="PATH üzerinde git / gh / docker görünmüyor."
        tools={systemTools}
        selected={selected}
        onToggle={toggle}
        onSelectAll={() => selectGroup(systemTools, true)}
      />

      <div className="flex items-center justify-between rounded-lg border border-outline-variant bg-surface-container px-3 py-2">
        <div className="font-mono text-body text-on-surface-variant">
          {selected.size} seçildi
        </div>
        <button
          type="button"
          onClick={() => void save()}
          disabled={saving || !report}
          className="rounded bg-primary-container px-3 py-1.5 text-xs font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
        >
          {saving ? "Kaydediliyor…" : "Sistemi Başlat"}
        </button>
      </div>
    </section>
  );
}

function ScanningSystem() {
  return (
    <section
      data-qa="panel"
      role="status"
      aria-live="polite"
      className="flex min-h-[calc(100vh-96px)] w-full flex-col items-center justify-center gap-4"
    >
      <BrandMark size={56} className="rounded-md" />
      <div className="font-mono text-xs font-bold tracking-[0.28em] text-on-surface uppercase">
        Scanning System...
      </div>
      <div className="font-mono text-meta text-outline">
        LMR :18790 · abonelik uygulamaları · plugin · PATH
      </div>
    </section>
  );
}

function SourceTable({ sources }: { sources: DiscoverySource[] }) {
  return (
    <table className="w-full table-fixed text-left font-mono text-body">
      <colgroup>
        <col className="w-[24%]" />
        <col className="w-[44%]" />
        <col className="w-[32%]" />
      </colgroup>
      <thead>
        <tr className="border-b border-outline-variant bg-surface-container-low/80 text-meta text-outline uppercase">
          <th className="px-3 py-1.5 font-medium">Kaynak</th>
          <th className="px-2 py-1.5 font-medium">Konum</th>
          <th className="px-3 py-1.5 font-medium">Durum</th>
        </tr>
      </thead>
      <tbody>
        {sources.map((source) => {
          const label = SOURCE_LABEL[source.id] ?? source.id;
          const title = SOURCE_TITLE[source.id] ?? label;
          const location = formatDiscoveryLocation(source.origin_path);
          const detail = humanizeDiscoveryDetail(source.detail);
          const install =
            !source.available && source.id !== "lmr" ? installFor(source.id) : null;
          return (
            <tr key={source.id} className="border-b border-outline-variant/40 last:border-b-0">
              <td className="px-3 py-2">
                <div className="flex min-w-0 items-center gap-2">
                  <Pip tone={source.available ? "ok" : "down"} />
                  <span className="truncate font-semibold text-on-surface" title={title}>
                    {label}
                  </span>
                </div>
              </td>
              <td className="min-w-0 px-2 py-2">
                <span className="block truncate text-on-surface-variant" title={source.origin_path ?? ""}>
                  {location}
                </span>
              </td>
              <td className="min-w-0 px-3 py-2">
                {source.available ? (
                  <span className="block truncate text-secondary" title={detail ?? "found"}>
                    {detail ?? "found"}
                  </span>
                ) : (
                  <span className="flex min-w-0 items-center gap-2 text-error">
                    <span className="truncate" title={detail ?? "yok"}>
                      {detail ?? "yok"}
                    </span>
                    {install ? (
                      <a
                        href={install.href}
                        target="_blank"
                        rel="noreferrer"
                        className="shrink-0 text-meta text-primary hover:underline"
                      >
                        {install.label}
                      </a>
                    ) : null}
                  </span>
                )}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function InstallLink({ label, href }: { label: string; href: string }) {
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className="mt-1 inline-flex items-center gap-1 font-mono text-meta text-primary hover:underline"
    >
      <Icon name="warn" className="h-3 w-3 text-error" />
      {label}
    </a>
  );
}

function LayaEngineBlock({ status }: { status: LayaEngineStatus | null }) {
  const percent = layaEnginePercentage(status);
  const downloading = status?.phase === "downloading";
  return (
    <div className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex items-start justify-between gap-3 border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <div className="min-w-0">
          <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
            OpenJev Laya
          </h2>
          <p className="mt-1 font-body text-body text-on-surface-variant">
            convaiinnovations/laya ağırlıkları uygulama dizinine indirilir; DecisionGate yalnızca
            doğrulanmış yerel dosyaları yükler.
          </p>
        </div>
        <span
          className={`shrink-0 font-mono text-body ${
            status?.phase === "failed"
              ? "text-error"
              : status?.phase === "ready"
                ? "text-secondary"
                : "text-on-surface-variant"
          }`}
        >
          {status?.label ?? "Laya Engine: Downloading"}
        </span>
      </div>
      {downloading || status?.phase === "failed" ? (
        <div className="px-3 py-2">
          <div className="flex items-center justify-between gap-2 font-mono text-meta text-on-surface-variant">
            <span className="truncate">{status?.error ?? status?.message ?? "hazırlanıyor"}</span>
            <span>{percent != null ? `${percent}%` : downloading ? "…" : ""}</span>
          </div>
          {downloading ? (
            <div className="mt-1 h-1.5 overflow-hidden rounded bg-surface-container-high">
              <div
                className={`h-full bg-primary ${percent == null ? "w-1/3 animate-pulse" : ""}`}
                style={{ width: percent != null ? `${percent}%` : undefined }}
              />
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function HfCatalogBlock({
  device,
  offers,
  selected,
  pulling,
  progress,
  onToggle,
  onPull,
}: {
  device: DeviceProfile | null;
  offers: HfModelOffer[];
  selected: Set<string>;
  pulling: boolean;
  progress: PullProgress | null;
  onToggle: (hfId: string, disabled: boolean) => void;
  onPull: () => void;
}) {
  const pending = offers.filter(
    (offer) => selected.has(offer.hf_id) && !offer.installed && !offer.heavy,
  ).length;
  const percent =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.completed / progress.total) * 100))
      : null;

  return (
    <div className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex items-start justify-between gap-3 border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <div className="min-w-0">
          <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
            Hugging Face · LMR
          </h2>
          <p className="mt-1 font-body text-body text-on-surface-variant">
            {device?.summary ?? "Cihaz profili okunuyor…"}
          </p>
        </div>
        <button
          type="button"
          onClick={onPull}
          disabled={pulling || pending === 0}
          className="shrink-0 rounded bg-primary-container px-2 py-1 font-mono text-body font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50"
        >
          {pulling ? "Çekiliyor…" : "LMR’ye çek"}
        </button>
      </div>
      {pulling || progress ? (
        <div className="border-b border-outline-variant px-3 py-2">
          <div className="flex items-center justify-between gap-2 font-mono text-meta text-on-surface-variant">
            <span className="truncate">{progress?.status ?? "hazırlanıyor"}</span>
            <span>{percent != null ? `${percent}%` : pulling ? "…" : ""}</span>
          </div>
          <div className="mt-1 h-1.5 overflow-hidden rounded bg-surface-container-high">
            <div
              className={`h-full bg-primary ${percent == null && pulling ? "w-1/3 animate-pulse" : ""}`}
              style={{ width: percent != null ? `${percent}%` : undefined }}
            />
          </div>
        </div>
      ) : null}
      {offers.length === 0 ? (
        <div className="px-3 py-4 font-mono text-body text-outline">
          Bu cihaz için önerilen GGUF bulunamadı.
        </div>
      ) : (
        <ul className="divide-y divide-outline-variant/40">
          {offers.map((offer) => {
            const disabled = offer.heavy && !offer.installed;
            const checked = offer.installed || selected.has(offer.hf_id);
            const org = offer.hf_id.split("/")[0] ?? offer.hf_id;
            return (
              <li key={offer.hf_id}>
                <label
                  className={`flex items-start gap-3 px-3 py-2 ${
                    disabled ? "opacity-60" : "cursor-pointer hover:bg-surface-container-high"
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={disabled || offer.installed || pulling}
                    onChange={() => onToggle(offer.hf_id, disabled || offer.installed)}
                    className="mt-0.5 accent-primary"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center justify-between gap-2 font-mono text-body">
                      <span className="font-semibold text-on-surface">{offer.name}</span>
                      <span className="shrink-0 text-outline">
                        {offer.params} · ~{offer.estimated_ram_gb.toFixed(1)} GB
                      </span>
                    </div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-1.5 font-mono text-meta text-on-surface-variant">
                      <span>{org}</span>
                      {offer.recommended ? (
                        <span className="rounded border border-secondary-container bg-secondary-container/40 px-1 text-secondary-dim">
                          önerilen
                        </span>
                      ) : null}
                      {offer.installed ? (
                        <span className="rounded border border-primary-container bg-primary-container/40 px-1 text-on-primary-container">
                          yüklü
                        </span>
                      ) : null}
                      {offer.heavy && !offer.installed ? (
                        <span className="rounded border border-error-container bg-error-container/30 px-1 text-error-dim">
                          cihaz için ağır
                        </span>
                      ) : null}
                    </div>
                    {offer.disabled_reason ? (
                      <div className="mt-0.5 font-body text-meta text-outline">
                        {offer.disabled_reason}
                      </div>
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

function ToolGroup({
  title,
  tools,
  selected,
  onToggle,
  onSelectAll,
  emptyHint,
  missing,
}: {
  title: string;
  tools: DiscoveredTool[];
  selected: Set<string>;
  onToggle: (id: string) => void;
  onSelectAll: () => void;
  emptyHint: string;
  missing?: { label: string; href: string } | null;
}) {
  return (
    <div className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
          {title}
          <span className="ml-2 font-medium text-outline">{tools.length}</span>
        </h2>
        {tools.some((tool) => tool.available) ? (
          <button
            type="button"
            onClick={onSelectAll}
            className="font-mono text-meta text-primary hover:underline"
          >
            Grubu seç
          </button>
        ) : null}
      </div>
      {tools.length === 0 ? (
        <div className="flex items-start gap-2 px-3 py-4">
          <Icon name="warn" className="mt-0.5 h-3.5 w-3.5 shrink-0 text-error" />
          <div>
            <div className="font-mono text-body text-outline">{emptyHint}</div>
            {missing ? <InstallLink {...missing} /> : null}
          </div>
        </div>
      ) : (
        <ul className="divide-y divide-outline-variant/40">
          {tools.map((tool) => {
            const checked = selected.has(tool.id);
            const install = !tool.available ? installFor(tool.name) ?? installFor(tool.source) : null;
            return (
              <li key={tool.id}>
                <label
                  className={`flex items-start gap-3 px-3 py-2 ${
                    tool.available ? "cursor-pointer hover:bg-surface-container-high" : "opacity-80"
                  }`}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={!tool.available}
                    onChange={() => onToggle(tool.id)}
                    className="mt-0.5 accent-primary"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center justify-between gap-2 font-mono text-body">
                      <span className="flex items-center gap-1.5 font-semibold text-on-surface">
                        {!tool.available ? <Icon name="warn" className="h-3 w-3 text-error" /> : null}
                        {tool.name}
                      </span>
                      <span className="min-w-0 shrink truncate text-right text-outline">
                        {hostLabelsFor(tool)}
                      </span>
                    </div>
                    <div className="mt-0.5 truncate font-mono text-meta text-on-surface-variant">
                      {tool.command
                        ? `${tool.command} ${tool.args.join(" ")}`.trim()
                        : (tool.endpoint ?? tool.origin_path ?? tool.id)}
                    </div>
                    {tool.detail ? (
                      <div className="mt-0.5 font-body text-meta text-outline">{tool.detail}</div>
                    ) : null}
                    {install && tool.source !== "lmr" ? <InstallLink {...install} /> : null}
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
