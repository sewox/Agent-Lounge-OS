"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { Icon } from "@/components/icons";
import { useLounge } from "@/components/lounge-provider";
import { eventToneClass, Kpi, LatencySparkline, outcomeClass, Pager, Pip, subjectClass } from "@/components/ui";
import {
  formatDecisionStreamLabel,
  formatExperienceTime,
  formatLayaDecision,
  formatLayaEngineFleetStatus,
  formatLatencyMs,
  layaEnginePercentage,
  MOCK_HEALTH,
  MOCK_NODES,
  natsEventTone,
  natsToneLabel,
  PAGE_SIZE,
  pageCount,
  pageSlice,
  quotaBarClass,
  quotaToneClass,
  type QuotaExhaustedAction,
  type QuotaKind,
  type SemanticProject,
} from "@/lib/lounge";

type SubjectFilter = "all" | "task" | "exp";
type QuotaFilter = "all" | QuotaKind;

function quotaKindClass(mode: string): string {
  if (mode === "subscription") {
    return "text-primary";
  }
  if (mode === "api") {
    return "text-tertiary";
  }
  if (mode === "plugin") {
    return "text-secondary";
  }
  return "text-on-surface-variant";
}

export function OverviewKpis() {
  const {
    experiences,
    projects,
    deadSymbols,
    lastIndex,
    semanticMap,
    indexing,
    decisionTelemetry,
    decisionLatencyHistory,
    decisionMsgPerMin,
    decisionGate,
  } = useLounge();
  const fromMap = semanticMap.projects.reduce((sum, row) => sum + row.files, 0);
  const fromProjects = projects.reduce((sum, row) => sum + (row.files ?? 0), 0);
  const indexedFiles = fromMap || fromProjects || lastIndex?.files || 0;
  const hasIndex =
    Boolean(lastIndex) || projects.length > 0 || semanticMap.projects.length > 0;
  const deadCount = hasIndex
    ? deadSymbols.length || lastIndex?.dead || 0
    : MOCK_HEALTH.reduce((sum, row) => sum + row.dead, 0);
  const latencyMs = decisionTelemetry?.latency_ms;
  const latencyLive = latencyMs != null && Number.isFinite(latencyMs);
  const latencyValue = latencyLive ? formatLatencyMs(latencyMs) : "—";
  const layaHint = formatLayaDecision(latencyLive ? latencyMs : null);
  const msgLive = decisionMsgPerMin > 0;
  return (
    <section className="shrink-0 space-y-2.5">
      {indexing ? (
        <div
          role="status"
          aria-live="polite"
          className="flex items-center gap-2 rounded-lg border border-outline-variant bg-surface-container px-3 py-2 font-mono text-[11px] text-on-surface"
        >
          <span
            className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-transparent border-t-primary"
            aria-hidden
          />
          <span className="font-bold tracking-wider uppercase">Scanning...</span>
          <span className="text-outline">Index Workspace</span>
        </div>
      ) : null}
      <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-5">
      <Kpi
        label="LATENCY"
        value={latencyValue}
        live={latencyLive}
        hint={
          decisionTelemetry?.device
            ? `${layaHint} · ${decisionTelemetry.device}`
            : layaHint
        }
        badge={
          latencyLive ? (
            <LatencySparkline values={decisionLatencyHistory} />
          ) : (
            <span className="font-mono text-[10px] text-on-surface-variant">
              {decisionGate?.phase === "ready" ? "idle" : "cold"}
            </span>
          )
        }
      />
      <Kpi
        label="MSG / MIN"
        value={String(decisionMsgPerMin)}
        live={msgLive}
        hint="NATS / 60s"
        badge={
          <span
            className={`flex items-center gap-0.5 rounded border px-1.5 py-0.5 font-mono text-[10px] font-medium ${
              msgLive
                ? "border-primary/30 bg-surface-container-high text-primary"
                : "border-outline-variant bg-surface-container-high text-on-surface-variant"
            }`}
          >
            {msgLive ? "live" : "idle"}
          </span>
        }
      />
      <Kpi
        label="INDEXED FILES"
        value={indexedFiles.toLocaleString("tr-TR")}
        hint={lastIndex?.project ? `${lastIndex.project} · memory_bridge` : "memory_bridge"}
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
      </div>
    </section>
  );
}

function DecisionStreamChip({
  label,
  live = false,
}: {
  label: string;
  live?: boolean;
}) {
  return (
    <span
      key={label}
      className={`kpi-tick shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] font-medium tnum ${
        live
          ? "border-primary/30 bg-primary-container/20 text-primary"
          : "border-primary/25 bg-surface-container-high text-primary"
      }`}
    >
      {label}
    </span>
  );
}

export function EventStreamPanel() {
  const { events, query, probeBus, decisionTelemetry } = useLounge();
  const [subjectFilter, setSubjectFilter] = useState<SubjectFilter>("all");
  const [probing, setProbing] = useState(false);
  const [page, setPage] = useState(0);
  const latencyMs = decisionTelemetry?.latency_ms;
  const decisionLive = latencyMs != null && Number.isFinite(latencyMs);
  const liveDecisionLabel = formatDecisionStreamLabel(decisionLive ? latencyMs : null);

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
      const haystack = `${event.subject} ${event.from} ${event.to} ${event.decisionLabel ?? ""}`.toLowerCase();
      return haystack.includes(query.trim().toLowerCase());
    });
  }, [events, query, subjectFilter]);

  const pages = pageCount(filtered.length);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(filtered, safePage);

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2.5">
          <Pip live tone="primary" />
          <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">NATS EVENT STREAM</h2>
          <span className="rounded bg-surface-container-high px-1 font-mono text-[10px] text-outline">topic: lounge.&gt;</span>
          <span className="rounded border border-primary/30 bg-primary-container/20 px-1.5 py-0.5 font-mono text-[10px] text-primary">
            bus live
          </span>
          {decisionLive ? <DecisionStreamChip label={liveDecisionLabel} live /> : null}
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
                onClick={() => {
                  setSubjectFilter(key);
                  setPage(0);
                }}
                className={`rounded px-2 py-0.5 ${subjectFilter === key ? "bg-primary-container font-medium text-on-primary-container" : "text-on-surface-variant hover:text-on-surface"}`}
              >
                {key === "all" ? "all" : key === "task" ? "task.*" : "exp.*"}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse text-left font-mono text-[11px]">
          <thead className="sticky top-0 z-10">
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/95 text-[10px] text-outline uppercase">
              <th className="w-[90px] px-2.5 py-1.5 font-medium">Time</th>
              <th className="px-2 py-1.5 font-medium">Subject</th>
              <th className="px-2 py-1.5 font-medium">Route</th>
              <th className="w-14 px-2 py-1.5 text-right font-medium">Payload</th>
              <th className="w-16 px-2.5 py-1.5 text-right font-medium">State</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-outline-variant/30">
            {visible.map((event, index) => {
              const selected = safePage === 0 && index === 0 && subjectFilter === "all" && !query;
              const tone = natsEventTone(event.subject, event.state);
              return (
                <tr
                  key={event.id}
                  className={
                    tone === "error"
                      ? "bg-error-container/10 hover:bg-error-container/20"
                      : tone === "task"
                        ? "border-l-2 border-primary bg-primary-container/15 hover:bg-primary-container/25"
                        : "bg-secondary-container/10 hover:bg-secondary-container/20"
                  }
                >
                  <td className="tnum px-2.5 py-1.5 text-on-surface-variant">{event.time}</td>
                  <td className={`px-2 py-1.5 ${subjectClass(event.subject, selected)}`}>
                    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                      <span className="min-w-0 truncate">{event.subject}</span>
                      {event.decisionLabel ? <DecisionStreamChip label={event.decisionLabel} live={selected} /> : null}
                    </div>
                  </td>
                  <td className="px-2 py-1.5 text-on-surface-variant">
                    {event.from} <span className="text-outline">→</span> {event.to}
                  </td>
                  <td className="tnum px-2.5 py-1.5 text-right text-on-surface-variant">{event.payload}</td>
                  <td className="px-2.5 py-1.5 text-right">
                    <span className={`rounded border px-1.5 py-0.5 font-mono text-[9px] uppercase ${eventToneClass(tone)}`}>
                      {natsToneLabel(tone)}
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
      <div className="flex shrink-0 items-center justify-between border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-[10px] text-outline">
        <span>
          {visible.length}/{filtered.length} · {PAGE_SIZE}/sayfa · {events.length} buffered
        </span>
        <div className="flex items-center gap-2">
          <Pager page={safePage} pages={pages} total={filtered.length} onPage={setPage} />
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
  const { experiences, projects, query, lastIndex, semanticMap } = useLounge();
  const nodes = semanticMap.projects.length
    ? semanticMap.projects.map((row) => ({
        name: row.name || "unnamed",
        edges: row.edge_count,
        modules: semanticModules(row),
      }))
    : projects.length
      ? projects.map((row) => ({
          name: row.name || "unnamed",
          edges: row.edges,
          modules: [
            `${row.files ?? 0} files`,
            `${row.nodes} nodes`,
            row.root_path ?? "sqlite+cbm",
          ],
        }))
      : MOCK_NODES;
  const fileTotal =
    semanticMap.projects.reduce((sum, row) => sum + row.files, 0) ||
    projects.reduce((sum, row) => sum + (row.files ?? 0), 0) ||
    lastIndex?.files ||
    0;
  const edgeTotal = nodes.reduce((sum, row) => sum + row.edges, 0);
  const log = experiences.filter((item) => {
    if (!query.trim()) {
      return true;
    }
    return `${item.project_id} ${item.adr_summary} ${item.agent}`.toLowerCase().includes(query.trim().toLowerCase());
  });

  const [logPage, setLogPage] = useState(0);
  const logPages = pageCount(log.length);
  const safeLogPage = Math.min(logPage, logPages - 1);
  const logVisible = pageSlice(log, safeLogPage);

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex shrink-0 items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
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
      <div className="flex min-h-0 flex-1 flex-col sm:flex-row">
        <div className="min-h-0 flex-1 space-y-2.5 overflow-auto border-b border-outline-variant bg-surface-container-low/40 p-2.5 font-mono text-[11px] sm:border-r sm:border-b-0">
          <div className="flex items-center justify-between text-[10px] font-semibold tracking-wider text-outline uppercase">
            <span>Indexed Files</span>
            <span className="text-on-surface-variant">
              {projects.length ? `${fileTotal} files · ${edgeTotal} edges` : `${edgeTotal} edges`}
            </span>
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
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden p-2.5 font-body">
          <div className="mb-2 flex shrink-0 items-center justify-between font-mono text-[10px] font-semibold tracking-wider text-outline uppercase">
            <span>Experience Log</span>
            <span className="text-secondary">Synced</span>
          </div>
          <div className="min-h-0 flex-1 space-y-2 overflow-auto font-mono text-[10.5px]">
            {logVisible.map((item) => (
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
      <div className="flex shrink-0 items-center justify-between border-t border-outline-variant bg-surface-container-low px-2.5 py-1.5 font-mono text-[10px] text-outline">
        <span>codebase-memory-mcp · {projects.length || nodes.length} repos</span>
        <Pager page={safeLogPage} pages={logPages} total={log.length} onPage={setLogPage} />
      </div>
    </section>
  );
}

export function HealthPanel() {
  const { projects, deadSymbols, semanticMap } = useLounge();
  const rows = semanticMap.projects.length
    ? semanticMap.projects.map((row) => ({
        name: row.name || "unnamed",
        indexed: row.node_count > 0 || row.files > 0 ? 100 : 0,
        files: String(row.files),
        nodes: String(row.node_count),
        stale: 0,
        dead: row.dead.length,
        sync: "live",
      }))
    : projects.length
    ? projects.map((row) => ({
        name: row.name || "unnamed",
        indexed: row.nodes > 0 || (row.files ?? 0) > 0 ? 100 : 0,
        files: String(row.files ?? 0),
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

  const [page, setPage] = useState(0);
  const pages = pageCount(rows.length);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(rows, safePage);

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex shrink-0 items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
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
      <div className="min-h-0 flex-1 space-y-2.5 overflow-auto p-2.5 font-mono text-[11px]">
        {visible.map((repo) => (
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
      <div className="flex shrink-0 items-center justify-end border-t border-outline-variant bg-surface-container-low px-3 py-1.5">
        <Pager page={safePage} pages={pages} total={rows.length} onPage={setPage} />
      </div>
    </section>
  );
}

export function QuotaPanel() {
  const { quotas, query, amberAlert, amberTools } = useLounge();
  const [quotaFilter, setQuotaFilter] = useState<QuotaFilter>("all");
  const rows = useMemo(() => {
    return quotas.filter((row) => {
      const mode = row.access_mode || row.kind;
      if (quotaFilter !== "all" && mode !== quotaFilter) {
        return false;
      }
      if (!query.trim()) {
        return true;
      }
      return `${row.tool} ${row.unit} ${mode} ${row.host_id ?? ""}`
        .toLowerCase()
        .includes(query.trim().toLowerCase());
    });
  }, [quotas, query, quotaFilter]);
  const [page, setPage] = useState(0);
  const pages = pageCount(rows.length);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(rows, safePage);
  const nearCap = quotas.filter((row) => row.percent !== null && (row.percent ?? 0) >= 80).length;

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex min-w-0 items-center gap-2.5">
          <Pip live tone="ok" />
          <h2 className="truncate font-mono text-xs font-bold tracking-wider text-on-surface uppercase">
            CONNECTED AI + BOT QUOTAS
          </h2>
          {amberAlert ? (
            <span className="rounded border border-error-container bg-error-container/20 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-error-dim uppercase">
              Amber Alert · {amberTools.join(", ") || "quota"}
            </span>
          ) : (
            <span className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-[10px] text-on-surface-variant">
              30s probes
            </span>
          )}
        </div>
        <div className="flex items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-mono text-[10px]">
          {(["all", "subscription", "api", "plugin", "local"] as const).map((key) => {
            return (
              <button
                key={key}
                type="button"
                onClick={() => {
                  setQuotaFilter(key);
                  setPage(0);
                }}
                className={`rounded px-2 py-0.5 ${quotaFilter === key ? "bg-primary-container font-medium text-on-primary-container" : "text-on-surface-variant hover:text-on-surface"}`}
              >
                {key}
              </button>
            );
          })}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full min-w-[720px] border-collapse text-left font-mono text-[11px]">
          <thead>
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/80 text-[10px] text-outline uppercase">
              <th className="px-2.5 py-1.5 font-medium">Tool</th>
              <th className="w-14 px-2 py-1.5 font-medium">Kind</th>
              <th className="w-16 px-2 py-1.5 font-medium">Unit</th>
              <th className="min-w-[160px] px-2 py-1.5 font-medium">Used / Limit</th>
              <th className="min-w-[160px] px-2 py-1.5 text-right font-medium">Remaining / Hosts</th>
              <th className="min-w-[140px] px-2 py-1.5 text-right font-medium">Reset</th>
              <th className="w-16 px-2.5 py-1.5 text-right font-medium">State</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-outline-variant/30">
            {visible.map((row) => (
              <tr
                key={row.id}
                className={
                  row.tone === "warn" || row.tone === "amber"
                    ? "bg-error-container/10 hover:bg-error-container/20"
                    : "hover:bg-surface-container-high/50"
                }
              >
                <td className="px-2.5 py-1.5 font-medium text-on-surface">{row.tool}</td>
                <td className={`px-2 py-1.5 font-medium ${quotaKindClass(row.access_mode || row.kind)}`}>
                  {(row.access_mode || row.kind).toUpperCase()}
                </td>
                <td className={`px-2 py-1.5 ${row.unit === "local" ? "text-secondary" : "text-on-surface-variant"}`}>
                  {row.unit}
                </td>
                <td className="px-2 py-1.5">
                  {row.percent === null ? (
                    <span className={row.tone === "live" ? "font-medium text-primary" : "tnum text-outline"}>{row.used}</span>
                  ) : (
                    <div className="flex items-center gap-2">
                      <span className="tnum text-on-surface">{row.used}</span>
                      <div className="h-1 w-16 shrink-0 overflow-hidden rounded-full bg-surface-container-highest">
                        <div className={`h-1 rounded-full ${quotaBarClass(row.percent)}`} style={{ width: `${row.percent}%` }} />
                      </div>
                    </div>
                  )}
                </td>
                <td className="px-2 py-1.5 text-right">
                  {row.access_mode === "plugin" || row.kind === "plugin" ? (
                    <div className="flex flex-wrap justify-end gap-1">
                      {row.remaining.split(" · ").filter(Boolean).map((host) => (
                        <span
                          key={host}
                          className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-[9px] text-on-surface-variant"
                        >
                          {host}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <span className="tnum text-on-surface-variant">{row.remaining}</span>
                  )}
                </td>
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
      <div className="flex shrink-0 items-center justify-between border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-[10px] text-outline">
        <span>abonelik: yerel plan · API keys · plugins · LMR sysinfo</span>
        <div className="flex items-center gap-3">
          <Pager page={safePage} pages={pages} total={rows.length} onPage={setPage} />
          <div className={`flex items-center gap-1.5 ${amberAlert ? "text-error-dim" : "text-outline"}`}>
            <span className={`h-1.5 w-1.5 rounded-full ${amberAlert ? "animate-pulse bg-error" : "bg-error"}`} />
            <span>{amberAlert ? "Amber Alert" : `${nearCap} tools near cap`}</span>
          </div>
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
    <section className="flex h-full min-h-0 min-w-0 flex-col overflow-auto">
      <div className="space-y-3">
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
          <div>
            <h2 className="font-mono text-xs font-bold tracking-wider text-on-surface uppercase">Bağlı araçlar</h2>
            <p className="mt-1 font-body text-[11px] text-on-surface-variant">
              Claude Desktop, Cursor uygulaması + plugin’leri, Antigravity, LMR ve Ollama yeniden taranır;
              seçim connected_tools tablosuna yazılır.
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
      </div>
    </section>
  );
}

export function FleetPanel() {
  const { report, model, decisionGate, layaEngine } = useLounge();
  const engineLabel = formatLayaEngineFleetStatus(layaEngine);
  const downloadPct = layaEnginePercentage(layaEngine);
  const gateHint =
    layaEngine?.phase === "downloading" && downloadPct != null
      ? `${downloadPct}%`
      : decisionGate?.phase === "ready"
        ? decisionGate.device || "DecisionGate"
        : "DecisionGate kapalı";
  const workers = [
    { id: "lounge-kernel", status: report?.ollama.running ? "ready" : "down", model },
    { id: "nats-hub", status: report?.nats.running ? "listening" : "down", model: "lounge.>" },
    { id: "memory-bridge", status: report?.memory.running ? "ready" : "missing", model: "cbm cli" },
    {
      id: "openjev-laya",
      status: engineLabel,
      model: gateHint,
    },
  ];
  const engineTone =
    layaEngine?.phase === "failed"
      ? "text-error"
      : layaEngine?.phase === "downloading"
        ? "text-on-surface-variant"
        : "text-secondary";
  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
      <div className="shrink-0 border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-mono text-xs font-bold tracking-wider uppercase">Worker Fleet</h2>
      </div>
      <div className="min-h-0 flex-1 divide-y divide-outline-variant/40 overflow-auto font-mono text-[11px]">
        {workers.map((row) => (
          <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2">
            <span className="text-on-surface">{row.id}</span>
            <span className="min-w-0 truncate text-on-surface-variant">{row.model}</span>
            <span
              className={
                row.id === "openjev-laya"
                  ? engineTone
                  : row.status === "down" || row.status === "missing"
                    ? "text-error"
                    : "text-secondary"
              }
            >
              {row.status}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

export function TelemetryPanel() {
  const {
    events,
    quotas,
    decisionTelemetry,
    decisionLatencyHistory,
    decisionMsgPerMin,
    decisionGate,
  } = useLounge();
  const subscription = quotas.filter((row) => (row.access_mode || row.kind) === "subscription");
  const plugins = quotas.filter((row) => (row.access_mode || row.kind) === "plugin");
  const latencyMs = decisionTelemetry?.latency_ms;
  const latencyLive = latencyMs != null && Number.isFinite(latencyMs);
  const msgLive = decisionMsgPerMin > 0;
  return (
    <section className="grid h-full min-h-0 min-w-0 auto-rows-fr gap-3 md:grid-cols-2">
      <div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-[11px]">
        <div className="text-[10px] tracking-wider text-outline uppercase">Laya Decision</div>
        <div
          key={latencyLive ? formatLatencyMs(latencyMs) : "idle"}
          className="kpi-tick mt-2 tnum text-2xl font-bold text-on-surface"
        >
          {latencyLive ? formatLatencyMs(latencyMs) : "—"}
        </div>
        <div className="text-outline">{formatLayaDecision(latencyLive ? latencyMs : null)}</div>
        <div className="mt-3 min-h-0 flex-1 overflow-auto">
          {latencyLive ? (
            <LatencySparkline values={decisionLatencyHistory} />
          ) : (
            <span className="text-[10px] text-on-surface-variant">
              {decisionGate?.phase === "ready" ? "idle · henüz infer yok" : "gate soğuk"}
            </span>
          )}
        </div>
        <div className="mt-auto pt-4 text-[10px] text-outline">
          lounge.telemetry.decision · {decisionTelemetry?.device ?? decisionGate?.device ?? "—"}
        </div>
      </div>
      <div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-[11px]">
        <div className="text-[10px] tracking-wider text-outline uppercase">MSG / MIN</div>
        <div
          key={decisionMsgPerMin}
          className="kpi-tick mt-2 tnum text-2xl font-bold text-on-surface"
        >
          {decisionMsgPerMin}
        </div>
        <div className="text-outline">NATS / 60s {msgLive ? "· live" : "· idle"}</div>
        <div className="mt-auto pt-4 text-[10px] text-outline">
          NATS buffer {events.length} · {subscription.length} abonelik · {plugins.length} plugin
        </div>
      </div>
    </section>
  );
}

function semanticModules(project: SemanticProject): string[] {
  const files = [
    ...new Set(
      project.nodes
        .map((node) => node.file)
        .filter((file): file is string => Boolean(file && file.trim())),
    ),
  ];
  const names = [...new Set(files.map((file) => file.split(/[/\\]/).filter(Boolean).at(-1) ?? file))];
  if (names.length === 0) {
    return [`${project.files} files`, `${project.node_count} nodes`, project.repo_path || "sqlite"];
  }
  return names.slice(0, 8);
}
