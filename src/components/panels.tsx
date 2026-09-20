"use client";

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { Icon } from "@/components/icons";
import { useLounge } from "@/components/lounge-provider";
import { eventStateClass, Kpi, outcomeClass, Pip, subjectClass } from "@/components/ui";
import {
  BUS_UI_EVENT,
  formatExperienceTime,
  isTauri,
  MOCK_HEALTH,
  MOCK_NODES,
  quotaBarClass,
  quotaToneClass,
  type LoungeMessage,
  type QuotaExhaustedAction,
  type QuotaKind,
} from "@/lib/lounge";

type SubjectFilter = "all" | "task" | "exp";
type QuotaFilter = "all" | QuotaKind;

export function OverviewKpis() {
  const { experiences, events, projects, deadSymbols } = useLounge();
  const files = projects.reduce((sum, row) => sum + row.nodes, 0);
  const deadCount = projects.length
    ? deadSymbols.length
    : MOCK_HEALTH.reduce((sum, row) => sum + row.dead, 0);
  return (
    <section className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-4">
      <Kpi
        label="MSG / MIN"
        value={String(Math.max(events.length, 1))}
        hint="buffered lounge.> events"
        badge={
          <span className="flex items-center gap-0.5 rounded border border-primary/30 bg-surface-container-high px-1.5 py-0.5 font-mono text-[10px] font-medium text-primary">
            live
          </span>
        }
      />
      <Kpi
        label="INDEXED NODES"
        value={files.toLocaleString("tr-TR")}
        hint="codebase-memory-mcp"
        badge={<span className="font-mono text-[10px] text-on-surface-variant">{projects.length} repos</span>}
      />
      <Kpi
        label="EXPERIENCES"
        value={String(experiences.length)}
        hint="vault persistence: sqlite"
        badge={<span className="font-mono text-[10px] text-secondary">synced</span>}
      />
      <Kpi
        label="DEAD SYMBOLS"
        value={String(deadCount)}
        hint="unreachable fn/struct refs"
        valueClass="text-error"
        badge={
          <span className="flex items-center gap-0.5 rounded border border-error-container bg-error-container/40 px-1.5 py-0.5 font-mono text-[10px] font-medium text-error-dim">
            ▼ amber alert
          </span>
        }
      />
    </section>
  );
}

export function EventStreamPanel() {
  const { events, query, ingestBusMessage, probeBus } = useLounge();
  const [subjectFilter, setSubjectFilter] = useState<SubjectFilter>("all");
  const [probing, setProbing] = useState(false);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void listen<LoungeMessage>(BUS_UI_EVENT, (event) => {
      if (!cancelled) {
        ingestBusMessage(event.payload);
      }
    }).then((fn) => {
      if (cancelled) {
        void fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      cancelled = true;
      if (unlisten) {
        void unlisten();
      }
    };
  }, [ingestBusMessage]);

  const filtered = useMemo(() => {
    return events.filter((event) => {
      if (subjectFilter === "task" && !event.subject.includes(".task.")) {
        return false;
      }
      if (subjectFilter === "exp" && !event.subject.includes("experience")) {
        return false;
      }
      if (!query.trim()) {
        return true;
      }
      const haystack = `${event.subject} ${event.from} ${event.to}`.toLowerCase();
      return haystack.includes(query.trim().toLowerCase());
    });
  }, [events, query, subjectFilter]);

  return (
    <section className="flex flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2.5">
          <Pip live tone="primary" />
          <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">NATS EVENT STREAM</h2>
          <span className="rounded bg-surface-container-high px-1 font-mono text-[10px] text-outline">topic: lounge.&gt;</span>
          <span className="rounded border border-primary/30 bg-primary-container/20 px-1.5 py-0.5 font-mono text-[10px] text-primary">
            bus live
          </span>
        </div>
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-1.5 rounded border border-outline-variant/60 bg-surface-container-high px-2 py-0.5">
            <span className="font-mono text-[10px] text-on-surface-variant">buffered:</span>
            <span className="tnum font-mono text-[10px] font-semibold text-primary">{events.length}</span>
          </div>
          <div className="flex items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-mono text-[10px]">
            {(["all", "task", "exp"] as const).map((key) => (
              <button
                key={key}
                type="button"
                onClick={() => setSubjectFilter(key)}
                className={`rounded px-2 py-0.5 ${subjectFilter === key ? "bg-primary-container font-medium text-on-primary-container" : "text-on-surface-variant hover:text-on-surface"}`}
              >
                {key === "all" ? "all" : key === "task" ? "task.*" : "exp.*"}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left font-mono text-[11px]">
          <thead>
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/80 text-[10px] text-outline uppercase">
              <th className="w-[90px] px-2.5 py-1.5 font-medium">Time</th>
              <th className="px-2 py-1.5 font-medium">Subject</th>
              <th className="px-2 py-1.5 font-medium">Route</th>
              <th className="w-14 px-2 py-1.5 text-right font-medium">Payload</th>
              <th className="w-16 px-2.5 py-1.5 text-right font-medium">State</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-outline-variant/30">
            {filtered.map((event, index) => {
              const selected = index === 0 && subjectFilter === "all" && !query;
              return (
                <tr
                  key={event.id}
                  className={
                    event.state === "error"
                      ? "bg-error-container/10 hover:bg-error-container/20"
                      : selected
                        ? "border-l-2 border-primary bg-primary-container/20 text-on-surface hover:bg-primary-container/30"
                        : "hover:bg-surface-container-high/50"
                  }
                >
                  <td className="tnum px-2.5 py-1.5 text-on-surface-variant">{event.time}</td>
                  <td className={`px-2 py-1.5 ${subjectClass(event.subject, selected)}`}>{event.subject}</td>
                  <td className="px-2 py-1.5 text-on-surface-variant">
                    {event.from} <span className="text-outline">→</span> {event.to}
                  </td>
                  <td className="tnum px-2 py-1.5 text-right text-on-surface-variant">{event.payload}</td>
                  <td className="px-2.5 py-1.5 text-right">
                    <span className={`rounded border px-1.5 py-0.5 font-mono text-[9px] ${eventStateClass(event.state)}`}>
                      {event.state}
                    </span>
                  </td>
                </tr>
              );
            })}
            {filtered.length === 0 ? (
              <tr>
                <td colSpan={5} className="px-2.5 py-6 text-center font-mono text-[11px] text-outline">
                  Bus dinleniyor — henüz lounge.&gt; mesajı yok
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="flex items-center justify-between border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-[10px] text-outline">
        <span>
          Streaming: {filtered.length} events visible / {events.length} buffered
        </span>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => {
              setProbing(true);
              void probeBus().finally(() => setProbing(false));
            }}
            className="rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 font-medium text-on-surface hover:bg-surface-bright"
          >
            {probing ? "Probing…" : "Probe bus"}
          </button>
          <span className="h-1.5 w-1.5 rounded-full bg-secondary" />
          <span className="text-on-surface-variant">NATS listen survives navigation</span>
        </div>
      </div>
    </section>
  );
}

export function VaultPanel() {
  const { experiences, projects, query } = useLounge();
  const nodes = projects.length
    ? projects.map((row) => ({
        name: row.name || "unnamed",
        edges: row.edges,
        modules: [`${row.nodes} nodes`, row.root_path ?? "sqlite+cbm"],
      }))
    : MOCK_NODES;
  const log = experiences.filter((item) => {
    if (!query.trim()) {
      return true;
    }
    return `${item.project_id} ${item.adr_summary} ${item.agent}`.toLowerCase().includes(query.trim().toLowerCase());
  });

  return (
    <section className="flex flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2">
          <span className="text-primary">
            <Icon name="tree" />
          </span>
          <h3 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
            SEMANTIC MAP + EXPERIENCES
          </h3>
        </div>
        <span className="font-mono text-[10px] text-outline">memory_bridge + sqlite</span>
      </div>
      <div className="flex min-h-[220px] flex-col sm:flex-row">
        <div className="space-y-2.5 border-b border-outline-variant bg-surface-container-low/40 p-2.5 font-mono text-[11px] sm:w-1/2 sm:border-r sm:border-b-0">
          <div className="flex items-center justify-between text-[10px] font-semibold tracking-wider text-outline uppercase">
            <span>Indexed Nodes</span>
            <span className="text-on-surface-variant">{nodes.reduce((sum, row) => sum + row.edges, 0)} edges</span>
          </div>
          {nodes.map((node, index) => (
            <div key={node.name}>
              <div className="flex items-center justify-between font-medium text-on-surface">
                <span className={`flex items-center gap-1 ${index === 0 ? "text-primary" : "text-on-surface"}`}>
                  <Icon name="folder" className="h-3 w-3" />
                  <span>{node.name}</span>
                </span>
                <span className="text-[10px] text-outline">{node.edges} edges</span>
              </div>
              <div className="mt-1 ml-1.5 space-y-0.5 border-l border-outline-variant/60 pl-3.5 text-[10px] text-on-surface-variant">
                {node.modules.map((mod) => (
                  <div key={mod}>↳ {mod}</div>
                ))}
              </div>
            </div>
          ))}
        </div>
        <div className="flex flex-col p-2.5 font-body sm:w-1/2">
          <div className="mb-2 flex items-center justify-between font-mono text-[10px] font-semibold tracking-wider text-outline uppercase">
            <span>Experience Log</span>
            <span className="text-secondary">Synced</span>
          </div>
          <div className="space-y-2 font-mono text-[10.5px]">
            {log.slice(0, 8).map((item) => (
              <div key={item.id} className="space-y-1 rounded border border-outline-variant/40 bg-surface-container-high/60 p-1.5">
                <div className="flex items-center justify-between">
                  <span className="tnum text-on-surface-variant">{formatExperienceTime(item.created_at)}</span>
                  <span className={`rounded border px-1 text-[9px] ${outcomeClass(item.outcome)}`}>
                    {item.outcome === "success" ? "Success" : item.outcome === "partial" ? "Partial" : "Failed"}
                  </span>
                </div>
                <div className="truncate text-[11px] font-medium text-on-surface">{item.project_id}</div>
                <div className="line-clamp-2 font-body text-[10px] leading-tight text-outline">“{item.adr_summary}”</div>
              </div>
            ))}
          </div>
        </div>
      </div>
      <div className="flex items-center justify-between border-t border-outline-variant bg-surface-container-low px-2.5 py-1.5 font-mono text-[10px] text-outline">
        <span>codebase-memory-mcp · {projects.length || nodes.length} repos</span>
        <span className="text-on-surface-variant">vector_dims: 256 lexical / 768 ollama</span>
      </div>
    </section>
  );
}

export function HealthPanel() {
  const { projects, deadSymbols } = useLounge();
  const rows = projects.length
    ? projects.map((row) => ({
        name: row.name || "unnamed",
        indexed: row.nodes > 0 ? 100 : 0,
        files: String(row.nodes),
        nodes: String(row.nodes),
        stale: 0,
        dead: deadSymbols.filter(
          (symbol) =>
            !symbol.project_id ||
            symbol.project_id === row.name ||
            symbol.project_id === row.root_path,
        ).length,
        sync: "live",
      }))
    : MOCK_HEALTH;

  return (
    <section className="flex flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2">
          <span className="text-primary">
            <Icon name="rule" />
          </span>
          <h3 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
            PROJECT HEALTH: INDEXING + DEAD CODE
          </h3>
        </div>
        <span className="font-mono text-[10px] text-outline">memory_bridge</span>
      </div>
      <div className="space-y-2.5 p-2.5 font-mono text-[11px]">
        {rows.map((repo) => (
          <div key={repo.name} className="space-y-1.5 rounded border border-outline-variant/40 bg-surface-container-high/40 p-2">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span className="font-bold text-on-surface">{repo.name}</span>
                <span
                  className={`rounded px-1 font-mono text-[9px] ${
                    repo.indexed === 100
                      ? "bg-secondary-container/50 text-secondary-dim"
                      : "bg-surface-container-highest text-tertiary"
                  }`}
                >
                  {repo.indexed}% indexed
                </span>
              </div>
              <div className="flex items-center gap-2 text-[10px] text-on-surface-variant">
                <span>sync: {repo.sync}</span>
                <span className="font-medium text-error">{repo.dead} dead symbols</span>
              </div>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-surface-container-highest">
              <div
                className={`h-1.5 rounded-full ${repo.indexed === 100 ? "bg-primary" : "bg-tertiary"}`}
                style={{ width: `${repo.indexed}%` }}
              />
            </div>
            <div className="flex items-center justify-between pt-0.5 text-[10px] text-outline">
              <span>
                {repo.files} files · {repo.nodes} AST nodes
              </span>
              <span>
                stale files:{" "}
                {repo.stale > 0 ? (
                  <strong className="rounded bg-error-container/30 px-1 text-error">{repo.stale}</strong>
                ) : (
                  <strong className="text-on-surface">0</strong>
                )}
              </span>
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}

export function QuotaPanel() {
  const { quotas, query } = useLounge();
  const [quotaFilter, setQuotaFilter] = useState<QuotaFilter>("all");
  const rows = useMemo(() => {
    return quotas.filter((row) => {
      if (quotaFilter !== "all" && row.kind !== quotaFilter) {
        return false;
      }
      if (!query.trim()) {
        return true;
      }
      return `${row.tool} ${row.unit} ${row.kind}`.toLowerCase().includes(query.trim().toLowerCase());
    });
  }, [quotas, query, quotaFilter]);
  const nearCap = quotas.filter((row) => row.percent !== null && (row.percent ?? 0) >= 70).length;

  return (
    <section className="flex flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2.5">
          <Pip live tone="ok" />
          <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
            CONNECTED AI + BOT QUOTAS
          </h2>
          <span className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-[10px] text-on-surface-variant">
            infra probes
          </span>
        </div>
        <div className="flex items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-mono text-[10px]">
          {(["all", "ai", "bots"] as const).map((key) => {
            const value: QuotaFilter = key === "bots" ? "bot" : key;
            return (
              <button
                key={key}
                type="button"
                onClick={() => setQuotaFilter(value)}
                className={`rounded px-2 py-0.5 ${quotaFilter === value ? "bg-primary-container font-medium text-on-primary-container" : "text-on-surface-variant hover:text-on-surface"}`}
              >
                {key}
              </button>
            );
          })}
        </div>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-left font-mono text-[11px]">
          <thead>
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/80 text-[10px] text-outline uppercase">
              <th className="px-2.5 py-1.5 font-medium">Tool</th>
              <th className="w-14 px-2 py-1.5 font-medium">Kind</th>
              <th className="w-16 px-2 py-1.5 font-medium">Unit</th>
              <th className="px-2 py-1.5 font-medium">Used / Limit</th>
              <th className="px-2 py-1.5 text-right font-medium">Remaining</th>
              <th className="w-20 px-2 py-1.5 text-right font-medium">Reset</th>
              <th className="w-24 px-2.5 py-1.5 text-right font-medium">State</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-outline-variant/30">
            {rows.map((row) => (
              <tr
                key={row.id}
                className={
                  row.tone === "warn"
                    ? "bg-error-container/10 hover:bg-error-container/20"
                    : "hover:bg-surface-container-high/50"
                }
              >
                <td className="px-2.5 py-1.5 font-medium text-on-surface">{row.tool}</td>
                <td className={`px-2 py-1.5 font-medium ${row.kind === "ai" ? "text-primary" : "text-secondary"}`}>
                  {row.kind.toUpperCase()}
                </td>
                <td className={`px-2 py-1.5 ${row.unit === "local" ? "text-secondary" : "text-on-surface-variant"}`}>
                  {row.unit}
                </td>
                <td className="px-2 py-1.5">
                  {row.percent === null ? (
                    <span className={row.tone === "live" ? "font-medium text-primary" : "tnum text-outline"}>{row.used}</span>
                  ) : (
                    <div className="flex items-center gap-2">
                      <span className="tnum min-w-[70px] text-on-surface">{row.used}</span>
                      <div className="h-1 w-24 shrink-0 overflow-hidden rounded-full bg-surface-container-highest">
                        <div className={`h-1 rounded-full ${quotaBarClass(row.percent)}`} style={{ width: `${row.percent}%` }} />
                      </div>
                    </div>
                  )}
                </td>
                <td className="tnum px-2 py-1.5 text-right text-on-surface-variant">{row.remaining}</td>
                <td className={`px-2 py-1.5 text-right ${row.reset === "LOCAL" ? "font-semibold text-secondary" : "tnum text-outline"}`}>
                  {row.reset}
                </td>
                <td className="px-2.5 py-1.5 text-right">
                  <span className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-[9px] ${quotaToneClass(row.tone)}`}>
                    {row.tone === "live" ? <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-secondary" /> : null}
                    {row.label}
                  </span>
                </td>
              </tr>
            ))}
            {rows.length === 0 ? (
              <tr>
                <td colSpan={7} className="px-2.5 py-6 text-center font-mono text-[11px] text-outline">
                  0 tools
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="flex items-center justify-between border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-[10px] text-outline">
        <span>quota source: infra/ LMR · NATS /varz · memory_bridge</span>
        <div className="flex items-center gap-1.5 text-error-dim">
          <span className="h-1.5 w-1.5 rounded-full bg-error" />
          <span>{nearCap} tools near cap</span>
        </div>
      </div>
    </section>
  );
}

const QUOTA_ACTIONS: { id: QuotaExhaustedAction; title: string; hint: string }[] = [
  { id: "stop", title: "Kotam biterse durdur", hint: "Görevi iptal et, ajan değiştirme." },
  { id: "ask_then_local", title: "Onay alarak yerel modele geç", hint: "Kota bitince UI onayı → LMR." },
  { id: "ask_then_abort", title: "Onay alarak iptal et", hint: "Kota bitince kullanıcı reddedebilir." },
];

export function SettingsPanel() {
  const { policy, savePolicy, model } = useLounge();

  return (
    <section className="space-y-3">
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
          <div>
            <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">Bağlı araçlar</h2>
            <p className="mt-1 font-body text-[11px] text-on-surface-variant">
              Claude Desktop, Cursor MCP, LMR ve Ollama yeniden taranır; seçim connected_tools tablosuna yazılır.
            </p>
          </div>
          <Link
            href="/onboarding"
            className="rounded bg-primary-container px-2.5 py-1 font-mono text-[11px] font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed"
          >
            Yeniden tara
          </Link>
        </div>
      </div>
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
          <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">Routing Policy</h2>
          <p className="mt-1 font-body text-[11px] text-on-surface-variant">
            Ajanlar arası otomatik geçiş kilitli. Dispatcher her görev öncesi bu politikayı ve kotaları kontrol eder.
          </p>
        </div>
        <div className="space-y-4 p-3">
          <label className="flex items-center justify-between rounded border border-outline-variant bg-surface-container-high px-3 py-2 font-mono text-[11px]">
            <span>Ajan geçişinde kullanıcı onayı</span>
            <input type="checkbox" checked disabled className="accent-primary" />
          </label>
          <div className="grid gap-2 md:grid-cols-3">
            {QUOTA_ACTIONS.map((action) => {
              const selected = policy.on_quota_exhausted === action.id;
              return (
                <button
                  key={action.id}
                  type="button"
                  onClick={() => void savePolicy({ ...policy, on_quota_exhausted: action.id })}
                  className={`rounded-lg border p-3 text-left ${
                    selected
                      ? "border-primary bg-primary-container/20"
                      : "border-outline-variant bg-surface-container-high hover:bg-surface-bright"
                  }`}
                >
                  <div className="font-mono text-[11px] font-semibold text-on-surface">{action.title}</div>
                  <div className="mt-1 font-body text-[10px] text-outline">{action.hint}</div>
                </button>
              );
            })}
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            <label className="space-y-1 font-mono text-[10px] text-on-surface-variant">
              Yerel fallback ajan
              <input
                value={policy.local_fallback_agent}
                onChange={(event) => void savePolicy({ ...policy, local_fallback_agent: event.target.value })}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1 text-[11px] text-on-surface"
              />
            </label>
            <label className="space-y-1 font-mono text-[10px] text-on-surface-variant">
              Yerel fallback model
              <input
                value={policy.local_fallback_model || model}
                onChange={(event) => void savePolicy({ ...policy, local_fallback_model: event.target.value })}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1 text-[11px] text-on-surface"
              />
            </label>
          </div>
          <div className="overflow-hidden rounded border border-outline-variant">
            <table className="w-full text-left font-mono text-[11px]">
              <thead>
                <tr className="border-b border-outline-variant bg-surface-container-low text-[10px] text-outline uppercase">
                  <th className="px-2.5 py-1.5">Ajan</th>
                  <th className="px-2 py-1.5">Tetik</th>
                  <th className="px-2.5 py-1.5 text-right">Enabled</th>
                </tr>
              </thead>
              <tbody>
                {policy.triggers.map((trigger, index) => (
                  <tr key={trigger.agent_id} className="border-b border-outline-variant/40">
                    <td className="px-2.5 py-1.5 text-on-surface">{trigger.label}</td>
                    <td className="px-2 py-1.5 text-on-surface-variant">{trigger.when}</td>
                    <td className="px-2.5 py-1.5 text-right">
                      <input
                        type="checkbox"
                        checked={trigger.enabled}
                        onChange={(event) => {
                          const triggers = policy.triggers.map((row, rowIndex) =>
                            rowIndex === index ? { ...row, enabled: event.target.checked } : row,
                          );
                          void savePolicy({ ...policy, triggers });
                        }}
                        className="accent-primary"
                      />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </div>
    </section>
  );
}

export function FleetPanel() {
  const { report, model } = useLounge();
  const workers = [
    { id: "lounge-kernel", status: report?.ollama.running ? "ready" : "down", model },
    { id: "nats-hub", status: report?.nats.running ? "listening" : "down", model: "lounge.>" },
    { id: "memory-bridge", status: report?.memory.running ? "ready" : "missing", model: "cbm cli" },
  ];
  return (
    <section className="rounded-lg border border-outline-variant bg-surface-container">
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-mono text-xs font-bold tracking-wider uppercase">Worker Fleet</h2>
      </div>
      <div className="divide-y divide-outline-variant/40 font-mono text-[11px]">
        {workers.map((row) => (
          <div key={row.id} className="flex items-center justify-between px-3 py-2">
            <span className="text-on-surface">{row.id}</span>
            <span className="text-on-surface-variant">{row.model}</span>
            <span className={row.status === "down" || row.status === "missing" ? "text-error" : "text-secondary"}>{row.status}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

export function TelemetryPanel() {
  const { events, quotas } = useLounge();
  return (
    <section className="grid gap-2.5 lg:grid-cols-2">
      <div className="rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-[11px]">
        <div className="text-[10px] tracking-wider text-outline uppercase">NATS buffer</div>
        <div className="mt-2 text-2xl font-bold text-on-surface">{events.length}</div>
        <div className="text-outline">events retained across routes</div>
      </div>
      <div className="rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-[11px]">
        <div className="text-[10px] tracking-wider text-outline uppercase">Quota probes</div>
        <div className="mt-2 text-2xl font-bold text-on-surface">{quotas.length}</div>
        <div className="text-outline">infra/ last snapshot</div>
      </div>
    </section>
  );
}
