"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { GraphUiButton } from "@/components/graph-ui-button";
import { GraphUiSettings } from "@/components/graph-ui-settings";
import { Icon } from "@/components/icons";
import { useLounge } from "@/components/lounge-provider";
import { SemanticMap } from "@/components/SemanticMap";
import { useUiScale } from "@/components/ui-scale-provider";
import { eventToneClass, Kpi, LatencySparkline, outcomeClass, Pager, Pip, subjectClass } from "@/components/ui";
import {
  buildBrowserEfficiencyReport,
  deadSymbolsMatchingSelection,
  downloadMarkdownFile,
  eventDecisionLabel,
  experiencesMatchingSelection,
  fetchAgentEfficiencyReport,
  formatDecisionStreamLabel,
  formatExperienceTime,
  formatLayaDecision,
  formatLayaEngineFleetStatus,
  formatLatencyMs,
  hasIndexedWorkspace,
  isHeartbeatSubject,
  isTauri,
  layaEnginePercentage,
  mergeClaudeQuotaRows,
  natsEventTone,
  natsToneLabel,
  PAGE_SIZE,
  pageCount,
  pageSlice,
  pathBasename,
  quotaBarClass,
  quotaToneClass,
  resolveDeadSymbolCount,
  type AgentEfficiencyReport,
  type QuotaExhaustedAction,
  type QuotaKind,
  type SemanticMapSelection,
} from "@/lib/lounge";
import { selectCriticalQuotas, type UiScale } from "@/lib/ui-prefs";
import { IndexEmptyState } from "@/components/index-empty-state";

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
  const deadCount = resolveDeadSymbolCount({
    deadSymbols,
    semanticMap,
    lastIndex,
    projects,
  });
  const indexed = hasIndexedWorkspace({ lastIndex, projects, semanticMap });
  const deadDisplay = indexed ? String(deadCount) : "—";
  const latencyMs = decisionTelemetry?.latency_ms;
  const latencyLive = latencyMs != null && Number.isFinite(latencyMs);
  const latencyValue = latencyLive ? formatLatencyMs(latencyMs) : "—";
  const layaHint = formatLayaDecision(latencyLive ? latencyMs : null);
  const msgLive = decisionMsgPerMin > 0;
  return (
    <section data-qa="panel" className="w-full shrink-0 space-y-2.5">
      {indexing ? (
        <div
          role="status"
          aria-live="polite"
          className="flex items-center gap-2 rounded-lg border border-outline-variant bg-surface-container px-3 py-2 font-body text-body text-on-surface"
        >
          <span
            className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-transparent border-t-primary"
            aria-hidden
          />
          <span className="font-semibold tracking-label uppercase">Scanning...</span>
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
            <span className="font-mono text-meta text-on-surface-variant">
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
            className={`flex items-center gap-0.5 rounded border px-1.5 py-0.5 font-body text-meta font-medium ${
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
        badge={<span className="font-mono text-meta text-on-surface-variant">{projects.length} repos</span>}
      />
      <Kpi
        label="EXPERIENCES"
        value={String(experiences.length)}
        hint="vault persistence: sqlite"
        badge={<span className="font-body text-meta text-secondary">synced</span>}
      />
      <Kpi
        label="DEAD SYMBOLS"
        value={deadDisplay}
        hint={indexed ? "unreachable fn/struct refs" : "Index Workspace"}
        valueClass={indexed && deadCount > 0 ? "text-error" : undefined}
        badge={
          indexed && deadCount > 0 ? (
            <span className="flex items-center gap-0.5 rounded border border-error-container bg-error-container/40 px-1.5 py-0.5 font-body text-meta font-medium text-error-dim">
              ▼ amber alert
            </span>
          ) : (
            <span className="font-mono text-meta text-on-surface-variant">
              {indexed ? "clean" : "no index"}
            </span>
          )
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
      className={`kpi-tick shrink-0 rounded border px-1.5 py-0.5 font-mono text-meta font-medium tnum ${
        live
          ? "border-primary/30 bg-primary-container/20 text-primary"
          : "border-primary/25 bg-surface-container-high text-primary"
      }`}
    >
      {label}
    </span>
  );
}

export function EventStreamPanel({ embedded = false }: { embedded?: boolean }) {
  const { events, query, probeBus, decisionTelemetry } = useLounge();
  const [subjectFilter, setSubjectFilter] = useState<SubjectFilter>("all");
  const [showHeartbeats, setShowHeartbeats] = useState(false);
  const [probing, setProbing] = useState(false);
  const [page, setPage] = useState(0);
  const latencyMs = decisionTelemetry?.latency_ms;
  const decisionLive = latencyMs != null && Number.isFinite(latencyMs);
  const liveDecisionLabel = formatDecisionStreamLabel(decisionLive ? latencyMs : null);
  const heartbeatCount = useMemo(
    () => events.filter((event) => isHeartbeatSubject(event.subject)).length,
    [events],
  );

  const filtered = useMemo(() => {
    return events.filter((event) => {
      if (!showHeartbeats && isHeartbeatSubject(event.subject)) {
        return false;
      }
      if (subjectFilter === "task" && !event.subject.includes(".task.")) {
        return false;
      }
      if (subjectFilter === "exp" && !event.subject.includes("experience")) {
        return false;
      }
      if (!query.trim()) {
        return true;
      }
      const haystack = `${event.subject} ${event.from} ${event.to} ${eventDecisionLabel(event, decisionLive ? latencyMs : null)} ${event.chainLabel ?? ""}`.toLowerCase();
      return haystack.includes(query.trim().toLowerCase());
    });
  }, [decisionLive, events, latencyMs, query, showHeartbeats, subjectFilter]);

  const pages = pageCount(filtered.length);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(filtered, safePage);

  const shell = embedded
    ? "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden bg-surface-container"
    : "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container";

  return (
    <section data-qa={embedded ? undefined : "panel"} className={shell}>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex min-w-0 flex-wrap items-center gap-2.5">
          <Pip live tone="primary" />
          {embedded ? null : (
            <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
              NATS EVENT STREAM
            </h2>
          )}
          <span className="rounded bg-surface-container-high px-1 font-mono text-meta text-outline">topic: lounge.&gt;</span>
          <span className="rounded border border-primary/30 bg-primary-container/20 px-1.5 py-0.5 font-body text-meta text-primary">
            bus live
          </span>
          {decisionLive ? <DecisionStreamChip label={liveDecisionLabel} live /> : null}
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center gap-1.5 rounded border border-outline-variant/60 bg-surface-container-high px-2 py-0.5">
            <span className="font-body text-meta text-on-surface-variant">buffered:</span>
            <span className="tnum font-mono text-meta font-semibold text-primary">{events.length}</span>
          </div>
          <button
            type="button"
            aria-pressed={showHeartbeats}
            onClick={() => {
              setShowHeartbeats((value) => !value);
              setPage(0);
            }}
            className={`min-h-8 rounded border px-2.5 py-1 font-body text-meta ${
              showHeartbeats
                ? "border-primary bg-primary-container/25 text-primary"
                : "border-outline-variant bg-surface-container-high text-on-surface-variant hover:text-on-surface"
            }`}
            title="Heartbeats are debug-level and hidden by default (SR-02)"
          >
            {showHeartbeats ? `Heartbeats on (${heartbeatCount})` : `Heartbeats hidden (${heartbeatCount})`}
          </button>
          <div className="flex items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-body text-meta">
            {(["all", "task", "exp"] as const).map((key) => (
              <button
                key={key}
                type="button"
                onClick={() => {
                  setSubjectFilter(key);
                  setPage(0);
                }}
                className={`min-h-8 rounded px-2.5 py-1 ${subjectFilter === key ? "bg-primary-container font-medium text-on-primary-container" : "text-on-surface-variant hover:text-on-surface"}`}
              >
                {key === "all" ? "all" : key === "task" ? "task.*" : "exp.*"}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse text-left font-body text-body">
          <thead className="sticky top-0 z-10">
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/95 font-body text-meta tracking-label text-outline uppercase">
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
              const decision = eventDecisionLabel(event, decisionLive ? latencyMs : null);
              const showDecision =
                Boolean(event.decisionLabel) ||
                (decisionLive && !isHeartbeatSubject(event.subject) && !decision.includes("—"));
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
                  <td className="tnum px-2.5 py-1.5 font-mono text-on-surface-variant">{event.time}</td>
                  <td className={`px-2 py-1.5 ${subjectClass(event.subject, selected)}`}>
                    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                      <span className="min-w-0 break-words font-mono">{event.subject}</span>
                      {showDecision ? (
                        <DecisionStreamChip
                          label={decision}
                          live={selected || Boolean(event.decisionLabel)}
                        />
                      ) : null}
                      {event.chainLabel ? (
                        <span
                          className="max-w-full break-words rounded border border-secondary/40 bg-secondary-container/30 px-1.5 py-0.5 font-mono text-meta font-medium text-secondary underline decoration-secondary/50 underline-offset-2"
                          title={event.chainLabel}
                          data-workflow-chain={event.chainLabel}
                        >
                          {event.chainLabel}
                        </span>
                      ) : null}
                    </div>
                  </td>
                  <td className="px-2 py-1.5 font-mono text-on-surface-variant">
                    {event.from} <span className="text-outline">→</span> {event.to}
                  </td>
                  <td className="tnum px-2.5 py-1.5 text-right font-mono text-on-surface-variant">{event.payload}</td>
                  <td className="px-2.5 py-1.5 text-right">
                    <span className={`rounded border px-1.5 py-0.5 font-body text-meta tracking-label uppercase ${eventToneClass(tone)}`}>
                      {natsToneLabel(tone)}
                    </span>
                  </td>
                </tr>
              );
            })}
            {filtered.length === 0 ? (
              <tr>
                <td colSpan={5} className="px-2.5 py-6 text-center font-body text-body text-outline">
                  {heartbeatCount > 0 && !showHeartbeats
                    ? "Heartbeats hidden — toggle to show debug traffic"
                    : "Bus dinleniyor — henüz lounge.> mesajı yok"}
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-body text-meta text-outline">
        <span>
          {visible.length}/{filtered.length} · {PAGE_SIZE}/sayfa · {events.length} buffered
        </span>
        <div className="flex flex-wrap items-center gap-2">
          <Pager page={safePage} pages={pages} total={filtered.length} onPage={setPage} />
          <button
            type="button"
            onClick={() => {
              setProbing(true);
              void probeBus().finally(() => setProbing(false));
            }}
            className="min-h-8 rounded border border-outline-variant bg-surface-container-high px-2.5 py-1 font-medium text-on-surface hover:bg-surface-bright"
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

export function VaultPanel({ embedded = false }: { embedded?: boolean }) {
  const {
    experiences,
    projects,
    query,
    lastIndex,
    semanticMap,
    deadSymbols,
    whisperedExperienceIds,
    selectedProject,
    switchProject,
    markWhisperUseful,
  } = useLounge();
  const [selected, setSelected] = useState<SemanticMapSelection | null>(null);
  const [selectionProject, setSelectionProject] = useState(selectedProject);
  const [markedUseful, setMarkedUseful] = useState<Set<string>>(() => new Set());
  const whispered = useMemo(() => new Set(whisperedExperienceIds), [whisperedExperienceIds]);

  if (selectionProject !== selectedProject) {
    setSelectionProject(selectedProject);
    if (selectedProject) {
      setSelected({
        id: `project:${selectedProject}`,
        name: selectedProject,
        kind: "project",
        project: selectedProject,
      });
    }
  }

  const fileTotal =
    semanticMap.projects.reduce((sum, row) => sum + row.files, 0) ||
    projects.reduce((sum, row) => sum + (row.files ?? 0), 0) ||
    lastIndex?.files ||
    0;
  const edgeTotal =
    semanticMap.projects.reduce((sum, row) => sum + row.edge_count, 0) ||
    projects.reduce((sum, row) => sum + row.edges, 0);
  const log = useMemo(
    () => experiencesMatchingSelection(experiences, selected, query),
    [experiences, selected, query],
  );
  const deadForNode = useMemo(
    () => deadSymbolsMatchingSelection(deadSymbols, semanticMap, selected),
    [deadSymbols, semanticMap, selected],
  );

  const [logPage, setLogPage] = useState(0);
  const logPages = pageCount(log.length);
  const safeLogPage = Math.min(logPage, logPages - 1);
  const logVisible = pageSlice(log, safeLogPage);
  const repoCount =
    semanticMap.projects.length || projects.length || (experiences.length ? 1 : 0);

  const shell = embedded
    ? "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden bg-surface-container"
    : "flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container";

  return (
    <section data-qa={embedded ? undefined : "panel"} className={shell}>
      <div className="flex shrink-0 items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2">
          <span className="text-primary">
            <Icon name="tree" />
          </span>
          {embedded ? null : (
            <h3 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
              SEMANTIC MAP + EXPERIENCES
            </h3>
          )}
        </div>
        <div className="flex items-center gap-2">
          <span className="font-body text-meta text-outline">
            {whispered.size > 0 ? (
              <span className="text-primary">whisper · live</span>
            ) : (
              "memory_bridge + sqlite"
            )}
          </span>
          <GraphUiButton />
        </div>
      </div>
      <div className="flex min-h-0 w-full flex-1 flex-col xl:flex-row">
        <div
          data-qa="panel"
          className="flex min-h-0 w-full flex-1 flex-col overflow-hidden border-b border-outline-variant bg-surface-container-low/40 p-2.5 xl:border-r xl:border-b-0"
        >
          <SemanticMap
            semanticMap={semanticMap}
            projects={projects}
            fileTotal={fileTotal}
            edgeTotal={edgeTotal}
            selected={selected}
            onSelect={(next) => {
              setSelected(next);
              setLogPage(0);
              if (next?.kind === "project") {
                switchProject(next.name);
              } else if (!next) {
                switchProject(null);
              }
            }}
          />
        </div>
        <div
          data-qa="panel"
          className="flex min-h-0 w-full flex-1 flex-col overflow-hidden p-2.5 font-body"
        >
          {selected ? (
            <div className="mb-2 shrink-0 space-y-1 border-b border-outline-variant/40 pb-2">
              <div className="flex items-center justify-between font-mono text-meta font-semibold tracking-wider text-outline uppercase">
                <span>Dead Symbols</span>
                <span className={deadForNode.length > 0 ? "text-error" : "text-outline"}>
                  {deadForNode.length > 0 ? `${deadForNode.length} uyarı` : "temiz"}
                </span>
              </div>
              {deadForNode.length === 0 ? (
                <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-2 text-center font-mono text-meta text-on-surface-variant">
                  Bu düğüm için dead symbol yok · {selected.name}
                </div>
              ) : (
                <div className="max-h-24 space-y-1 overflow-auto font-mono text-meta">
                  {deadForNode.slice(0, 8).map((symbol) => (
                    <div
                      key={`${symbol.name}:${symbol.file ?? ""}:${symbol.line ?? ""}:${symbol.kind}`}
                      className="flex items-start justify-between gap-2 rounded border border-error/30 bg-error/5 px-1.5 py-1"
                    >
                      <div className="min-w-0">
                        <div className="truncate font-medium text-error">{symbol.name}</div>
                        <div className="truncate text-meta text-on-surface-variant">
                          {symbol.detail ||
                            (symbol.file
                              ? `${pathBasename(symbol.file) || symbol.file}${
                                  symbol.line != null ? `:${symbol.line}` : ""
                                }`
                              : symbol.kind)}
                        </div>
                      </div>
                      <span className="shrink-0 rounded border border-error/40 px-1 text-meta text-error uppercase">
                        {symbol.kind || "dead"}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : null}
          <div className="mb-2 flex shrink-0 items-center justify-between font-body text-meta font-semibold tracking-label text-outline uppercase">
            <span>Experience Log</span>
            <span
              className={
                whispered.size > 0 ? "text-primary" : selected ? "text-primary" : "text-secondary"
              }
            >
              {whispered.size > 0
                ? `fısıltı · ${whispered.size}`
                : selected
                  ? `filter · ${selected.name}`
                  : "Synced"}
            </span>
          </div>
          <div className="min-h-0 flex-1 space-y-2 overflow-auto font-body text-body">
            {logVisible.length === 0 ? (
              <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center text-body text-on-surface-variant">
                {selected
                  ? `Bu düğüm için experience yok · ${selected.name}`
                  : "Experience kaydı yok"}
              </div>
            ) : (
              logVisible.map((item) => {
                const isWhisper = whispered.has(item.id);
                const alreadyUseful = markedUseful.has(item.id);
                return (
                  <div
                    key={item.id}
                    className={`space-y-1 rounded border p-1.5 transition-colors ${
                      isWhisper
                        ? "border-primary bg-primary-container/20 shadow-[0_0_0_1px_var(--color-primary)]"
                        : "border-outline-variant/40 bg-surface-container-high/60"
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="tnum font-mono text-meta text-on-surface-variant">
                        {formatExperienceTime(item.created_at)}
                      </span>
                      <span className="flex items-center gap-1">
                        {isWhisper ? (
                          <span className="rounded border border-primary/50 px-1 text-meta text-primary">
                            WHISPER
                          </span>
                        ) : null}
                        <span
                          className={`rounded border px-1 text-meta ${outcomeClass(item.outcome)}`}
                        >
                          {item.outcome === "success"
                            ? "Success"
                            : item.outcome === "partial"
                              ? "Partial"
                              : "Failed"}
                        </span>
                      </span>
                    </div>
                    <div className="break-words font-mono text-body font-medium text-on-surface">
                      {item.project_id}
                    </div>
                    <div className="line-clamp-3 font-body text-body leading-normal text-outline">
                      “{item.adr_summary}”
                    </div>
                    {isWhisper ? (
                      <div className="flex justify-end pt-0.5">
                        <button
                          type="button"
                          disabled={alreadyUseful}
                          onClick={(event) => {
                            event.stopPropagation();
                            void (async () => {
                              await markWhisperUseful(item.id, item.project_id);
                              setMarkedUseful((prev) => new Set(prev).add(item.id));
                            })();
                          }}
                          className="min-h-8 rounded border border-primary/40 px-1.5 py-1 font-body text-meta text-primary hover:bg-primary-container/30 disabled:cursor-default disabled:opacity-60"
                          aria-label="Bu fısıltıyı faydalı olarak işaretle"
                        >
                          {alreadyUseful ? "Faydalı ✓" : "Faydalı"}
                        </button>
                      </div>
                    ) : null}
                  </div>
                );
              })
            )}
          </div>
        </div>
      </div>
      <div className="flex shrink-0 items-center justify-between border-t border-outline-variant bg-surface-container-low px-2.5 py-1.5 font-mono text-meta text-outline">
        <span>codebase-memory-mcp · {repoCount} repos</span>
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
    : [];

  const [page, setPage] = useState(0);
  const pages = pageCount(rows.length);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(rows, safePage);

  return (
    <section
      data-qa="panel"
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="flex shrink-0 items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex items-center gap-2">
          <span className="text-primary">
            <Icon name="rule" />
          </span>
          <h3 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
            PROJECT HEALTH: INDEXING + DEAD CODE
          </h3>
        </div>
        <span className="font-mono text-meta text-outline">memory_bridge</span>
      </div>
      <div className="min-h-0 flex-1 space-y-2.5 overflow-auto p-2.5 font-mono text-body">
        {rows.length === 0 ? (
          <IndexEmptyState detail="No data found." className="min-h-full" />
        ) : (
          visible.map((repo) => (
            <div key={repo.name} className="w-full space-y-1.5 rounded border border-outline-variant/40 bg-surface-container-high/40 p-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <span className="font-bold text-on-surface">{repo.name}</span>
                  <span
                    className={`rounded px-1 font-mono text-meta ${
                      repo.indexed === 100
                        ? "bg-secondary-container/50 text-secondary-dim"
                        : "bg-surface-container-highest text-tertiary"
                    }`}
                  >
                    {repo.indexed}% indexed
                  </span>
                </div>
                <div className="flex items-center gap-2 text-meta text-on-surface-variant">
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
              <div className="flex items-center justify-between pt-0.5 text-meta text-outline">
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
          ))
        )}
      </div>
      <div className="flex shrink-0 items-center justify-end border-t border-outline-variant bg-surface-container-low px-3 py-1.5">
        <Pager page={safePage} pages={pages} total={rows.length} onPage={setPage} />
      </div>
    </section>
  );
}

export function QuotaMiniCard() {
  const { quotas, quotaError, amberAlert, amberTools } = useLounge();
  const displayQuotas = useMemo(() => mergeClaudeQuotaRows(quotas), [quotas]);
  const critical = useMemo(() => selectCriticalQuotas(displayQuotas, 3), [displayQuotas]);

  return (
    <div className="space-y-2 p-2.5" data-testid="quota-mini-card">
      {amberAlert ? (
        <div className="rounded border border-error-container bg-error-container/20 px-2.5 py-1.5 font-body text-meta text-error-dim">
          Amber Alert · {amberTools.join(", ") || "quota"}
        </div>
      ) : null}
      {critical.length === 0 ? (
        <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2.5 py-4 text-center font-body text-body text-outline">
          {quotaError ?? "Kota verisi yok"}
        </div>
      ) : (
        critical.map((row) => (
          <div
            key={row.id}
            data-quota-id={row.id}
            className={`space-y-1.5 rounded border px-2.5 py-2 ${
              row.tone === "warn" || row.tone === "amber" || row.exhausted
                ? "border-error-container/60 bg-error-container/10"
                : "border-outline-variant/50 bg-surface-container-high/50"
            }`}
          >
            <div className="flex items-center justify-between gap-2">
              <span className="min-w-0 truncate font-body text-body font-semibold text-on-surface">
                {row.tool}
              </span>
              <span
                className={`inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-0.5 font-body text-meta tracking-label uppercase ${quotaToneClass(row.tone)}`}
              >
                {row.label}
              </span>
            </div>
            <div className="flex items-center gap-2">
              <span className="tnum shrink-0 font-mono text-meta text-on-surface-variant">
                {row.percent === null ? row.used : `${Math.round(row.percent)}%`}
              </span>
              {row.percent !== null ? (
                <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-container-highest">
                  <div
                    className={`h-1.5 rounded-full ${quotaBarClass(row.percent)}`}
                    style={{ width: `${Math.min(100, Math.max(0, row.percent))}%` }}
                  />
                </div>
              ) : (
                <span className="min-w-0 truncate font-mono text-meta text-outline">{row.remaining}</span>
              )}
            </div>
          </div>
        ))
      )}
      <div className="pt-1 text-right">
        <Link
          href="/quotas"
          className="font-body text-meta font-medium text-primary hover:underline"
        >
          /quotas sayfasında tüm liste
        </Link>
      </div>
    </div>
  );
}

export function QuotaPanel() {
  const { quotas, quotaError, query, amberAlert, amberTools } = useLounge();
  const [quotaFilter, setQuotaFilter] = useState<QuotaFilter>("all");
  const displayQuotas = useMemo(() => mergeClaudeQuotaRows(quotas), [quotas]);
  const rows = useMemo(() => {
    return displayQuotas.filter((row) => {
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
  }, [displayQuotas, query, quotaFilter]);
  const [page, setPage] = useState(0);
  const pages = pageCount(rows.length);
  const safePage = Math.min(page, pages - 1);
  const visible = pageSlice(rows, safePage);
  const nearCap = displayQuotas.filter((row) => row.percent !== null && (row.percent ?? 0) >= 80).length;

  return (
    <section
      data-qa="panel"
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex min-w-0 items-center gap-2.5">
          <Pip live tone="ok" />
          <h2 className="truncate font-body text-panel font-semibold tracking-label text-on-surface uppercase">
            CONNECTED AI + BOT QUOTAS
          </h2>
          {amberAlert ? (
            <span className="rounded border border-error-container bg-error-container/20 px-1.5 py-0.5 font-mono text-meta font-semibold text-error-dim uppercase">
              Amber Alert · {amberTools.join(", ") || "quota"}
            </span>
          ) : (
            <span className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-meta text-on-surface-variant">
              30s probes
            </span>
          )}
        </div>
        <div className="flex flex-wrap items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-mono text-meta">
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
      <div className="min-h-0 w-full flex-1 overflow-x-auto overflow-y-auto">
        <table className="w-full min-w-0 border-collapse text-left font-mono text-body table-fixed md:table-auto">
          <thead>
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/80 text-meta text-outline uppercase">
              <th className="px-2.5 py-1.5 font-medium">Tool</th>
              <th className="w-14 px-2 py-1.5 font-medium">Kind</th>
              <th className="hidden w-16 px-2 py-1.5 font-medium sm:table-cell">Unit</th>
              <th className="px-2 py-1.5 font-medium">Used / Limit</th>
              <th className="hidden px-2 py-1.5 text-right font-medium md:table-cell">Remaining / Hosts</th>
              <th className="hidden px-2 py-1.5 text-right font-medium lg:table-cell">Reset</th>
              <th className="w-16 px-2.5 py-1.5 text-right font-medium">State</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-outline-variant/30">
            {visible.map((row) => (
              <tr
                key={row.id}
                data-quota-id={row.id}
                data-quota-tool={row.tool}
                className={
                  row.tone === "warn" || row.tone === "amber"
                    ? "bg-error-container/10 hover:bg-error-container/20"
                    : "hover:bg-surface-container-high/50"
                }
              >
                <td className="truncate px-2.5 py-1.5 font-medium text-on-surface">{row.tool}</td>
                <td className={`px-2 py-1.5 font-medium ${quotaKindClass(row.access_mode || row.kind)}`}>
                  {(row.access_mode || row.kind).toUpperCase()}
                </td>
                <td className={`hidden px-2 py-1.5 sm:table-cell ${row.unit === "local" ? "text-secondary" : "text-on-surface-variant"}`}>
                  {row.unit}
                </td>
                <td className="px-2 py-1.5">
                  {row.percent === null ? (
                    <span className={row.tone === "live" ? "font-medium text-primary" : "tnum text-outline"}>{row.used}</span>
                  ) : (
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="tnum shrink-0 text-on-surface">{row.used}</span>
                      <div className="h-1 w-12 min-w-0 flex-1 overflow-hidden rounded-full bg-surface-container-highest sm:w-16 sm:flex-none">
                        <div className={`h-1 rounded-full ${quotaBarClass(row.percent)}`} style={{ width: `${row.percent}%` }} />
                      </div>
                    </div>
                  )}
                </td>
                <td className="hidden px-2 py-1.5 text-right md:table-cell">
                  {row.access_mode === "plugin" || row.kind === "plugin" ? (
                    <div className="flex flex-wrap justify-end gap-1">
                      {row.remaining.split(" · ").filter(Boolean).map((host) => (
                        <span
                          key={host}
                          className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-meta text-on-surface-variant"
                        >
                          {host}
                        </span>
                      ))}
                    </div>
                  ) : (
                    <span className="tnum text-on-surface-variant">{row.remaining}</span>
                  )}
                </td>
                <td className={`hidden px-2 py-1.5 text-right lg:table-cell ${row.reset === "LOCAL" ? "font-semibold text-secondary" : "tnum text-outline"}`}>
                  {row.reset}
                </td>
                <td className="px-2.5 py-1.5 text-right">
                  <span className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-meta ${quotaToneClass(row.tone)}`}>
                    {row.tone === "live" ? <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-secondary" /> : null}
                    {row.label}
                  </span>
                </td>
              </tr>
            ))}
            {rows.length === 0 ? (
              <tr>
                <td colSpan={7} className="px-2.5 py-6 text-center font-mono text-body text-outline">
                  {quotaError ?? "0 tools"}
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-meta text-outline">
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
  const { scale, setScale, scales } = useUiScale();

  const scaleLabel = (value: UiScale) => `${Math.round(value * 100)}%`;

  return (
    <section data-qa="panel" className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-auto">
      <div className="w-full space-y-3">
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
          <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">Routing Policy</h2>
          <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
            Ajanlar arası otomatik geçiş kilitli. Dispatcher her görev öncesi bu politikayı ve kotaları kontrol eder.
          </p>
        </div>
        <div className="space-y-3 p-3">
          <label
            className="flex items-center justify-between rounded border border-outline-variant bg-surface-container-high px-3 py-2 font-body text-body"
            title="Always required — cannot be disabled (security)"
          >
            <span>Ajan geçişinde kullanıcı onayı (kilitli)</span>
            <input
              type="checkbox"
              checked
              disabled
              title="Always required — cannot be disabled (security)"
              className="accent-primary"
            />
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
                  <div className="font-body text-body font-semibold text-on-surface">{action.title}</div>
                  <div className="mt-1 font-body text-meta leading-normal text-outline">{action.hint}</div>
                </button>
              );
            })}
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            <label className="space-y-1 font-body text-meta text-on-surface-variant">
              Yerel fallback ajan
              <input
                value={policy.local_fallback_agent}
                onChange={(event) => void savePolicy({ ...policy, local_fallback_agent: event.target.value })}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-mono text-body text-on-surface"
              />
            </label>
            <label className="space-y-1 font-body text-meta text-on-surface-variant">
              Yerel fallback model
              <input
                value={policy.local_fallback_model || model}
                onChange={(event) => void savePolicy({ ...policy, local_fallback_model: event.target.value })}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-mono text-body text-on-surface"
              />
            </label>
          </div>
          <div
            data-qa="routing-table"
            className="max-h-[min(14rem,40vh)] overflow-auto rounded border border-outline-variant"
          >
            <table className="w-full text-left font-body text-body">
              <thead className="sticky top-0 z-[1]">
                <tr className="border-b border-outline-variant bg-surface-container-low text-meta tracking-label text-outline uppercase">
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
          <p className="font-body text-meta text-outline">Scroll · routing triggers</p>
        </div>
      </div>
      <GraphUiSettings />
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
          <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
            UI ölçeği / UI scale
          </h2>
          <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
            Kök font boyutunu (rem) ölçekler. Kısayollar: Cmd/Ctrl + büyüt, Cmd/Ctrl − küçült,
            Cmd/Ctrl 0 → %100.
          </p>
        </div>
        <div className="flex flex-wrap gap-2 p-3" role="radiogroup" aria-label="UI ölçeği">
          {scales.map((value) => {
            const selected = scale === value;
            return (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={selected}
                onClick={() => setScale(value)}
                className={`min-h-8 rounded border px-3 py-1.5 font-body text-body ${
                  selected
                    ? "border-primary bg-primary-container/25 font-semibold text-primary"
                    : "border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright"
                }`}
              >
                {scaleLabel(value)}
              </button>
            );
          })}
        </div>
      </div>
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex items-center justify-between border-b border-outline-variant bg-surface-container-low p-2.5">
          <div>
            <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">Bağlı araçlar</h2>
            <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
              Claude Desktop, Cursor uygulaması + plugin’leri, Antigravity, LMR ve Ollama yeniden taranır;
              seçim connected_tools tablosuna yazılır.
            </p>
          </div>
          <Link
            href="/onboarding"
            className="min-h-8 rounded bg-primary-container px-2.5 py-1.5 font-body text-body font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed"
          >
            Yeniden tara
          </Link>
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
  const [natsWorkers, setNatsWorkers] = useState<
    { id: string; label: string; status: string; model: string }[]
  >([]);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const load = async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const rows = await invoke<
          {
            id: string;
            name: string;
            kind: string;
            is_active: boolean;
            enabled: boolean;
            payload: { available?: boolean; detail?: string };
            endpoint?: string | null;
          }[]
        >("list_connected_tools");
        if (cancelled) return;
        setNatsWorkers(
          rows
            .filter((row) => row.kind === "worker")
            .map((row) => {
              const online =
                row.is_active &&
                row.enabled &&
                (row.payload?.available ?? false);
              return {
                id: row.id,
                label: row.name || row.id,
                status: online ? "online" : "offline",
                model: row.endpoint || row.payload?.detail || "nats worker",
              };
            }),
        );
      } catch {
        if (!cancelled) setNatsWorkers([]);
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 8_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const workers = [
    { id: "lounge-kernel", label: "lounge-kernel", status: report?.ollama.running ? "ready" : "down", model: model ?? "" },
    { id: "nats-hub", label: "nats-hub", status: report?.nats.running ? "listening" : "down", model: "lounge.>" },
    { id: "memory-bridge", label: "memory-bridge", status: report?.memory.running ? "ready" : "missing", model: "cbm cli" },
    {
      id: "openjev-laya",
      label: "openjev-laya",
      status: engineLabel,
      model: gateHint,
    },
    ...natsWorkers,
  ];
  const engineTone =
    layaEngine?.phase === "failed"
      ? "text-error"
      : layaEngine?.phase === "downloading"
        ? "text-on-surface-variant"
        : "text-secondary";
  return (
    <section
      data-qa="panel"
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="shrink-0 border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label uppercase">Worker Fleet</h2>
      </div>
      <div className="min-h-0 w-full flex-1 divide-y divide-outline-variant/40 overflow-auto font-mono text-body">
        {workers.map((row) => (
          <div key={row.id} className="flex w-full items-center justify-between gap-2 px-3 py-2.5">
            <span className="min-w-0 flex-1 truncate text-on-surface">{row.label}</span>
            <span className="min-w-0 max-w-[40%] flex-1 truncate text-right text-on-surface-variant">{row.model}</span>
            <span
              className={`shrink-0 ${
                row.id === "openjev-laya"
                  ? engineTone
                  : row.status === "down" ||
                      row.status === "missing" ||
                      row.status === "offline"
                    ? "text-error"
                    : "text-secondary"
              }`}
            >
              {row.status}
            </span>
          </div>
        ))}
        <div className="space-y-2 px-3 py-4 font-body text-body text-on-surface-variant">
          <p className="font-semibold text-on-surface">Fleet status</p>
          <p>
            Workers report via NATS heartbeat. Offline workers flip within ~45s after the last
            ping. Use Index Workspace on Health when local graph data is missing.
          </p>
          <p className="font-mono text-meta text-outline">
            lounge.workers.heartbeat · DecisionGate · memory-bridge
          </p>
          <p className="font-mono text-meta text-outline">
            Ensure core daemons show Running in the sidebar before dispatching tasks.
          </p>
        </div>
      </div>
      <div className="flex shrink-0 items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-2 font-body text-meta text-outline">
        <span className="font-body">{workers.length} workers registered</span>
        <span className="font-mono">nats · supervisor</span>
      </div>
    </section>
  );
}

export function TelemetryPanel() {
  const {
    events,
    quotas,
    experiences,
    deadSymbols,
    projects,
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

  const [weekly, setWeekly] = useState(true);
  const [projectId, setProjectId] = useState<string>("");
  const [tauriReport, setTauriReport] = useState<AgentEfficiencyReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tauriHost = isTauri();

  const projectOptions = useMemo(() => {
    const ids = new Set<string>();
    for (const p of projects) {
      if (p.name) ids.add(p.name);
    }
    for (const e of experiences) {
      if (e.project_id) ids.add(e.project_id);
    }
    return [...ids].sort();
  }, [projects, experiences]);

  const browserReport = useMemo(() => {
    if (tauriHost) return null;
    return buildBrowserEfficiencyReport({
      events,
      experiences,
      deadSymbols,
      weekly,
      projectId: projectId || null,
    });
  }, [tauriHost, weekly, projectId, events, experiences, deadSymbols]);

  const report = tauriHost ? tauriReport : browserReport;

  useEffect(() => {
    if (!tauriHost) return;
    let cancelled = false;
    const load = async () => {
      setLoading(true);
      setError(null);
      try {
        const next = await fetchAgentEfficiencyReport({
          weekly,
          projectId: projectId || null,
        });
        if (!cancelled) setTauriReport(next);
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setTauriReport(
            buildBrowserEfficiencyReport({
              events,
              experiences,
              deadSymbols,
              weekly,
              projectId: projectId || null,
            }),
          );
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [tauriHost, weekly, projectId, events, experiences, deadSymbols]);

  const rateLabel =
    report?.dead.rate != null ? `${(report.dead.rate * 100).toFixed(1)}%` : "—";

  return (
    <section data-qa="panel" className="flex h-full min-h-0 w-full min-w-0 flex-col gap-3 overflow-hidden">
      <div className="grid shrink-0 gap-3 md:grid-cols-2">
        <div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-body">
          <div className="text-meta tracking-wider text-outline uppercase">Laya Decision</div>
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
              <span className="text-meta text-on-surface-variant">
                {decisionGate?.phase === "ready" ? "idle · henüz infer yok" : "gate soğuk"}
              </span>
            )}
          </div>
          <div className="mt-auto pt-4 text-meta text-outline">
            lounge.telemetry.decision · {decisionTelemetry?.device ?? decisionGate?.device ?? "—"}
          </div>
        </div>
        <div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-body">
          <div className="text-meta tracking-wider text-outline uppercase">MSG / MIN</div>
          <div
            key={decisionMsgPerMin}
            className="kpi-tick mt-2 tnum text-2xl font-bold text-on-surface"
          >
            {decisionMsgPerMin}
          </div>
          <div className="text-outline">NATS / 60s {msgLive ? "· live" : "· idle"}</div>
          <div className="mt-auto pt-4 text-meta text-outline">
            NATS buffer {events.length} · {subscription.length} abonelik · {plugins.length} plugin
          </div>
        </div>
      </div>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-outline-variant bg-surface-container-low px-3 py-2">
          <h2 className="font-body text-panel font-semibold tracking-label uppercase">
            Agent Efficiency Report
          </h2>
          <span className="text-meta text-outline">Ajan Verimlilik Raporu</span>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-1.5 font-mono text-meta text-on-surface-variant">
              <input
                type="checkbox"
                checked={weekly}
                onChange={(e) => setWeekly(e.target.checked)}
                className="accent-primary"
              />
              Haftalık (7g)
            </label>
            <select
              value={projectId}
              onChange={(e) => setProjectId(e.target.value)}
              className="max-w-[10rem] truncate rounded border border-outline-variant bg-surface px-1.5 py-1 font-mono text-meta text-on-surface"
              aria-label="Proje filtresi"
            >
              <option value="">Tüm projeler</option>
              {projectOptions.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>
            <button
              type="button"
              disabled={!report}
              onClick={() => {
                if (!report) return;
                const stamp = report.generatedAt.slice(0, 10);
                downloadMarkdownFile(`agent-efficiency-${stamp}.md`, report.markdown);
              }}
              className="rounded border border-outline-variant bg-surface-container-high px-2 py-1 font-mono text-meta font-bold tracking-wider text-on-surface uppercase enabled:hover:bg-surface-container disabled:opacity-40"
            >
              Markdown indir
            </button>
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3 font-mono text-body">
          {loading && !report ? (
            <p className="text-on-surface-variant">Rapor yükleniyor…</p>
          ) : null}
          {error ? (
            <p className="mb-2 text-meta text-error">Tauri rapor hatası · mock gösteriliyor: {error}</p>
          ) : null}
          {report ? (
            <div className="space-y-4">
              <p className="text-meta text-outline">
                {report.scopeLabel}
                {isTauri() ? "" : " · tarayıcı"}
                {loading ? " · yenileniyor…" : ""}
              </p>
              <div className="grid gap-2 sm:grid-cols-3">
                <div className="rounded border border-outline-variant/60 bg-surface-container-low px-2.5 py-2">
                  <div className="text-meta tracking-wider text-outline uppercase">
                    Toplam failure
                  </div>
                  <div className="tnum mt-1 text-xl font-bold text-on-surface">
                    {report.totalFailures}
                  </div>
                  <div className="text-meta text-on-surface-variant">ajan × hata/failure</div>
                </div>
                <div className="rounded border border-outline-variant/60 bg-surface-container-low px-2.5 py-2">
                  <div className="text-meta tracking-wider text-outline uppercase">
                    Cross-Project
                  </div>
                  <div className="tnum mt-1 text-xl font-bold text-on-surface">
                    {report.crossProjectExperienceHits}
                  </div>
                  <div className="text-meta text-on-surface-variant">
                    {report.crossProjectWhisperEvents} fısıltı olayı
                  </div>
                </div>
                <div className="rounded border border-outline-variant/60 bg-surface-container-low px-2.5 py-2">
                  <div className="text-meta tracking-wider text-outline uppercase">
                    Dead cleanup
                  </div>
                  <div className="tnum mt-1 text-xl font-bold text-on-surface">{rateLabel}</div>
                  <div className="text-meta text-on-surface-variant">
                    kalan {report.dead.remaining}
                    {report.dead.cleaned != null ? ` · temizlenen ${report.dead.cleaned}` : ""}
                  </div>
                </div>
              </div>

              <div>
                <h3 className="mb-1.5 text-meta font-bold tracking-wider text-outline uppercase">
                  Ajan başına hata / failure
                </h3>
                {report.failuresByAgent.length === 0 ? (
                  <p className="text-on-surface-variant">Kayıt yok.</p>
                ) : (
                  <table className="w-full text-left">
                    <thead>
                      <tr className="text-meta text-outline">
                        <th className="py-1 font-normal">Ajan</th>
                        <th className="py-1 text-right font-normal">Sayı</th>
                      </tr>
                    </thead>
                    <tbody>
                      {report.failuresByAgent.map((row) => (
                        <tr key={row.agentId} className="border-t border-outline-variant/40">
                          <td className="py-1.5 text-on-surface">{row.agentId}</td>
                          <td className="tnum py-1.5 text-right text-on-surface">
                            {row.failureCount}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>

              <div className="text-meta text-on-surface-variant">
                <div className="mb-1 font-bold tracking-wider text-outline uppercase">Kaynak</div>
                <ul className="list-inside list-disc space-y-0.5">
                  {report.sourceNotes.map((note) => (
                    <li key={note}>{note}</li>
                  ))}
                </ul>
                <p className="mt-2 text-outline">{report.dead.note}</p>
              </div>
            </div>
          ) : null}
        </div>
      </div>
      <div className="flex shrink-0 items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-2">
        <span className="font-body text-meta text-outline">
          Efficiency · {report ? report.scopeLabel : "waiting"}
        </span>
        <span className="font-mono text-meta text-outline">
          buffer {events.length} · dead {deadSymbols.length}
        </span>
      </div>
    </section>
  );
}
