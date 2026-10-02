"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { GraphUiButton } from "@/components/graph-ui-button";
import { GraphUiSettings } from "@/components/graph-ui-settings";
import { Icon } from "@/components/icons";
import { useLounge } from "@/components/lounge-provider";
import { ExperienceDrawer } from "@/components/experience-drawer";
import { LocaleSwitch } from "@/components/locale-switch";
import { ApprovalSoundSettingsPanel } from "@/components/approval-sound-settings";
import { DestructiveGateSettingsPanel } from "@/components/destructive-gate-settings";
import { EditorSettingsPanel } from "@/components/editor-settings-panel";
import { ExperienceGovernanceSettingsPanel } from "@/components/experience-governance-settings";
import { SemanticMap } from "@/components/SemanticMap";
import {
  formatDisplayPath,
  formatExperienceLogSummary,
  isExperienceArchived,
  resolveGraphTotals,
  sortExperiencesForDisplay,
} from "@/lib/experience";
import { vaultStrings as vaultS } from "@/lib/strings/vault";
import { useIsTauri } from "@/hooks/use-is-tauri";
import { useUiScale } from "@/components/ui-scale-provider";
import { eventToneClass, Kpi, LatencySparkline, outcomeClass, Pager, Pip, subjectClass } from "@/components/ui";
import {
  buildBrowserEfficiencyReport,
  deadSymbolsMatchingSelection,
  eventDecisionLabel,
  experiencesMatchingSelection,
  fetchAgentEfficiencyReport,
  fleetHealthFields,
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
  saveMarkdownReport,
  sortEventsNewestFirst,
  natsEventTone,
  natsToneLabel,
  PAGE_SIZE,
  pageCount,
  pageSlice,
  quotaBarClass,
  quotaToneClass,
  resolveDeadSymbolCount,
  type AgentEfficiencyReport,
  type QuotaExhaustedAction,
  type QuotaKind,
  type SemanticMapSelection,
} from "@/lib/lounge";
import { selectCriticalQuotas, type UiScale } from "@/lib/ui-prefs";
import { DeadSymbolsVaultLink } from "@/components/dead-symbols-panel";
import { IndexEmptyState } from "@/components/index-empty-state";
import { deadSymbolStrings as dsStrings } from "@/lib/strings/dead-symbols";

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
  const { t } = useTranslation("dashboard");
  const { t: tStream } = useTranslation("stream");
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
          <span className="font-semibold tracking-label uppercase">{t("scanning")}</span>
          <span className="text-outline">{t("indexWorkspace")}</span>
        </div>
      ) : null}
      <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-5">
      <Kpi
        label={t("kpi.latency")}
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
              {decisionGate?.phase === "ready" ? t("idle") : t("cold")}
            </span>
          )
        }
      />
      <Kpi
        label={t("kpi.msgPerMin")}
        value={String(decisionMsgPerMin)}
        live={msgLive}
        hint={tStream("kpiNats60s")}
        badge={
          <span
            className={`flex items-center gap-0.5 rounded border px-1.5 py-0.5 font-body text-meta font-medium ${
              msgLive
                ? "border-primary/30 bg-surface-container-high text-primary"
                : "border-outline-variant bg-surface-container-high text-on-surface-variant"
            }`}
          >
            {msgLive ? t("live") : t("idle")}
          </span>
        }
      />
      <Kpi
        label={t("kpi.indexedFiles")}
        value={indexedFiles.toLocaleString("en-US")}
        hint={
          lastIndex?.project
            ? `${lastIndex.project} · ${tStream("kpiMemoryBridge")}`
            : tStream("kpiMemoryBridge")
        }
        badge={
          <span className="font-mono text-meta text-on-surface-variant">
            {t("repos", { count: projects.length })}
          </span>
        }
      />
      <Kpi
        label={t("kpi.experiences")}
        value={String(experiences.length)}
        hint={tStream("kpiVaultPersistence")}
        badge={<span className="font-body text-meta text-secondary">{t("synced")}</span>}
      />
      <Kpi
        label={t("kpi.deadSymbols")}
        value={deadDisplay}
        hint={indexed ? t("unreachableRefs") : t("indexWorkspace")}
        valueClass={indexed && deadCount > 0 ? "text-error" : undefined}
        badge={
          indexed && deadCount > 0 ? (
            <span className="flex items-center gap-0.5 rounded border border-error-container bg-error-container/40 px-1.5 py-0.5 font-body text-meta font-medium text-error-dim">
              {t("amberAlert")}
            </span>
          ) : (
            <span className="font-mono text-meta text-on-surface-variant">
              {indexed ? t("clean") : t("noIndex")}
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
  const { t } = useTranslation("stream");
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
    const rows = events.filter((event) => {
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
    return sortEventsNewestFirst(rows);
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
              {t("title")}
            </h2>
          )}
          <span className="rounded bg-surface-container-high px-1 font-mono text-meta text-outline">{t("topic")}</span>
          <span className="rounded border border-primary/30 bg-primary-container/20 px-1.5 py-0.5 font-body text-meta text-primary">
            {t("busLive")}
          </span>
          {decisionLive ? <DecisionStreamChip label={liveDecisionLabel} live /> : null}
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center gap-1.5 rounded border border-outline-variant/60 bg-surface-container-high px-2 py-0.5">
            <span className="font-body text-meta text-on-surface-variant">{t("buffered")}</span>
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
            title={t("heartbeatsTitle")}
          >
            {showHeartbeats
              ? t("heartbeatsOn", { count: heartbeatCount })
              : t("heartbeatsHidden", { count: heartbeatCount })}
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
                {key === "all" ? t("filterAll") : key === "task" ? t("filterTask") : t("filterExp")}
              </button>
            ))}
          </div>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full table-fixed border-collapse text-left font-body text-body">
          <thead className="sticky top-0 z-10">
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/95 font-body text-meta tracking-label text-outline uppercase">
              <th className="w-[6.5rem] px-2.5 py-1.5 font-medium">{t("colTime")}</th>
              <th className="px-2 py-1.5 font-medium">{t("colSubject")}</th>
              <th className="w-[11rem] px-2 py-1.5 font-medium">{t("colRoute")}</th>
              <th className="w-14 px-2 py-1.5 text-right font-medium">{t("colPayload")}</th>
              <th className="w-16 px-2.5 py-1.5 text-right font-medium">{t("colState")}</th>
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
              const routeLabel = `${event.from} → ${event.to}`;
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
                  <td className="tnum whitespace-nowrap px-2.5 py-1.5 font-mono text-on-surface-variant">
                    {event.time}
                  </td>
                  <td className={`min-w-0 px-2 py-1.5 ${subjectClass(event.subject, selected)}`}>
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
                  <td className="w-[11rem] max-w-[11rem] px-2 py-1.5">
                    <span
                      className="block truncate whitespace-nowrap font-mono text-on-surface-variant"
                      title={routeLabel}
                    >
                      {routeLabel}
                    </span>
                  </td>
                  <td className="tnum whitespace-nowrap px-2.5 py-1.5 text-right font-mono text-on-surface-variant">
                    {event.payload}
                  </td>
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
                    ? t("emptyHeartbeatsHidden")
                    : t("emptyListening")}
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-body text-meta text-outline">
        <span>
          {t("footerPage", {
            visible: visible.length,
            filtered: filtered.length,
            pageSize: PAGE_SIZE,
            buffered: events.length,
          })}
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
            {probing ? t("probing") : t("probeBus")}
          </button>
          <span className="h-1.5 w-1.5 rounded-full bg-secondary" />
          <span className="text-on-surface-variant">{t("listenSurvives")}</span>
        </div>
      </div>
    </section>
  );
}

export function VaultPanel({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation("dashboard");
  const {
    experiences,
    projects,
    query,
    semanticMap,
    deadSymbols,
    whisperedExperienceIds,
    selectedProject,
    switchProject,
    markWhisperUseful,
    showArchived,
    setShowArchived,
    unreviewedCount,
    experienceTotal,
    experiencesLoading,
    experiencesError,
    markAllExperiencesReviewed,
    loadMoreExperiences,
    openExperience,
    experienceDrawerOpen,
    experienceDetail,
    experienceDetailLoading,
    experienceDetailError,
    closeExperience,
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

  const graphTotals = useMemo(
    () => resolveGraphTotals({ projects, semanticMap }),
    [projects, semanticMap],
  );
  const selectedProjectPath = useMemo(() => {
    if (!selected || selected.kind !== "project") {
      return null;
    }
    const fromProjects = projects.find((row) => row.name === selected.name)?.root_path;
    const fromMap = semanticMap.projects.find((row) => row.name === selected.name)?.repo_path;
    return fromProjects || fromMap || null;
  }, [projects, selected, semanticMap.projects]);
  const log = useMemo(
    () =>
      sortExperiencesForDisplay(
        experiencesMatchingSelection(experiences, selected, query),
      ),
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
              {t("panels.vault")}
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
          className="flex min-h-0 w-full min-w-0 flex-1 flex-col overflow-auto border-b border-outline-variant bg-surface-container-low/40 p-2.5 xl:border-r xl:border-b-0"
        >
          <SemanticMap
            semanticMap={semanticMap}
            projects={projects}
            graphTotals={graphTotals}
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
          <div
            data-qa="vault-experience-header"
            className="mb-2 flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-outline-variant/40 pb-2"
          >
            <div className="font-mono text-meta text-on-surface-variant">
              {vaultS.totalExperiences(experienceTotal)}
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={() => setShowArchived(!showArchived)}
                className="min-h-8 rounded border border-outline-variant px-2 py-1 font-body text-meta text-on-surface-variant hover:bg-surface-container-high"
              >
                {showArchived ? vaultS.hideArchived : vaultS.showArchived}
              </button>
              {unreviewedCount > 0 ? (
                <button
                  type="button"
                  onClick={() => void markAllExperiencesReviewed()}
                  className="min-h-8 rounded border border-secondary/40 px-2 py-1 font-body text-meta text-secondary hover:bg-secondary/10"
                >
                  {vaultS.markAllReviewed}
                </button>
              ) : null}
            </div>
          </div>
          {selectedProjectPath ? (
            <div
              data-qa="vault-project-path"
              className="mb-2 shrink-0 break-all font-mono text-meta text-on-surface-variant"
            >
              <span className="uppercase tracking-label text-outline">{vaultS.projectPath}</span>
              {" · "}
              {formatDisplayPath(selectedProjectPath)}
            </div>
          ) : null}
          {selected ? (
            <div className="mb-2 shrink-0 space-y-1 border-b border-outline-variant/40 pb-2">
              <div className="flex items-center justify-between font-mono text-meta font-semibold tracking-wider text-outline uppercase">
                <span>{vaultS.deadSymbols}</span>
                <span className={deadForNode.length > 0 ? "text-error" : "text-outline"}>
                  {deadForNode.length > 0 ? vaultS.warnings(deadForNode.length) : vaultS.clean}
                </span>
              </div>
              {deadForNode.length === 0 ? (
                <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-2 text-center font-mono text-meta text-on-surface-variant">
                  Bu düğüm için dead symbol yok · {selected.name}
                </div>
              ) : (
                <div className="space-y-1">
                  <div className="max-h-24 space-y-1 overflow-auto font-mono text-meta">
                    {deadForNode.slice(0, 3).map((symbol) => (
                      <div
                        key={`${symbol.name}:${symbol.file ?? ""}:${symbol.line ?? ""}:${symbol.kind}`}
                        className="flex items-start justify-between gap-2 rounded border border-error/30 bg-error/5 px-1.5 py-1"
                      >
                        <div className="min-w-0">
                          <div className="truncate font-medium text-error">{symbol.name}</div>
                          <div className="truncate text-meta text-on-surface-variant">
                            {symbol.file
                              ? `${formatDisplayPath(symbol.file)}${
                                  symbol.line != null ? `:${symbol.line}` : ""
                                }`
                              : symbol.detail || symbol.kind}
                          </div>
                        </div>
                        <span className="shrink-0 rounded border border-error/40 px-1 text-meta uppercase text-error">
                          {symbol.kind || "dead"}
                        </span>
                      </div>
                    ))}
                  </div>
                  <DeadSymbolsVaultLink
                    count={deadForNode.length}
                    project={selected.kind === "project" ? selected.name : selected.project}
                  />
                </div>
              )}
            </div>
          ) : null}
          <div className="mb-2 flex shrink-0 items-center justify-between font-body text-meta font-semibold tracking-label text-outline uppercase">
            <span>{vaultS.experienceLog}</span>
            <span
              className={
                whispered.size > 0 ? "text-primary" : selected ? "text-primary" : "text-secondary"
              }
            >
              {whispered.size > 0
                ? vaultS.whisperLive(whispered.size)
                : selected
                  ? vaultS.filter(selected.name)
                  : vaultS.synced}
            </span>
          </div>
          <div className="min-h-0 flex-1 space-y-2 overflow-auto font-body text-body">
            {experiencesLoading && experiences.length === 0 ? (
              <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center text-body text-on-surface-variant">
                {vaultS.loading}
              </div>
            ) : experiencesError ? (
              <div className="rounded border border-error/40 bg-error/5 px-2 py-4 text-center text-body text-error">
                {experiencesError || vaultS.listError}
              </div>
            ) : logVisible.length === 0 ? (
              <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2 py-4 text-center text-body text-on-surface-variant">
                {selected
                  ? vaultS.noExperiencesForNode(selected.name)
                  : vaultS.noExperiences}
              </div>
            ) : (
              logVisible.map((item) => {
                const isWhisper = whispered.has(item.id);
                const alreadyUseful = markedUseful.has(item.id);
                const archived = isExperienceArchived(item);
                const summary = formatExperienceLogSummary(item.adr_summary);
                return (
                  <button
                    key={item.id}
                    type="button"
                    data-qa="experience-card"
                    onClick={() => void openExperience(item.id)}
                    className={`w-full space-y-1 rounded border p-1.5 text-left transition-colors ${
                      isWhisper
                        ? "border-primary bg-primary-container/20 shadow-[0_0_0_1px_var(--color-primary)]"
                        : archived
                          ? "border-outline-variant/30 bg-surface-container-high/30 opacity-70"
                          : "border-outline-variant/40 bg-surface-container-high/60 hover:bg-surface-container-high"
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="tnum font-mono text-meta text-on-surface-variant">
                        {formatExperienceTime(item.created_at)}
                      </span>
                      <span className="flex items-center gap-1">
                        {item.is_pinned ? (
                          <span className="rounded border border-primary/40 px-1 text-meta text-primary">
                            PIN
                          </span>
                        ) : null}
                        {archived ? (
                          <span className="rounded border border-outline-variant px-1 text-meta text-outline uppercase">
                            {vaultS.archivedLabel}
                          </span>
                        ) : null}
                        {item.reviewed === false ? (
                          <span className="rounded border border-secondary/40 px-1 text-meta text-secondary uppercase">
                            {vaultS.unreviewed}
                          </span>
                        ) : null}
                        {isWhisper ? (
                          <span className="rounded border border-primary/50 px-1 text-meta text-primary">
                            {vaultS.whisper}
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
                    <div className="font-body text-body leading-normal text-outline">{summary}</div>
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
                          {alreadyUseful ? vaultS.usefulDone : vaultS.useful}
                        </button>
                      </div>
                    ) : null}
                  </button>
                );
              })
            )}
          </div>
          {experiences.length < experienceTotal ? (
            <div className="shrink-0 pt-2">
              <button
                type="button"
                data-qa="vault-load-more"
                disabled={experiencesLoading}
                onClick={() => void loadMoreExperiences()}
                className="min-h-8 w-full rounded border border-outline-variant px-2 py-1 font-body text-meta text-on-surface-variant hover:bg-surface-container-high disabled:opacity-60"
              >
                {vaultS.loadMore}
              </button>
            </div>
          ) : null}
        </div>
      </div>
      <div className="flex shrink-0 items-center justify-between border-t border-outline-variant bg-surface-container-low px-2.5 py-1.5 font-mono text-meta text-outline">
        <span>
          codebase-memory-mcp · {repoCount} repos · {vaultS.totalExperiences(experienceTotal)}
        </span>
        <Pager page={safeLogPage} pages={logPages} total={log.length} onPage={setLogPage} />
      </div>
      <ExperienceDrawer
        key={experienceDetail?.id ?? "closed"}
        open={experienceDrawerOpen}
        experience={experienceDetail}
        loading={experienceDetailLoading}
        error={experienceDetailError}
        onClose={closeExperience}
      />
    </section>
  );
}

export function HealthPanel() {
  const { t } = useTranslation("health");
  const { projects, deadSymbols, semanticMap, indexing, indexWorkspace } = useLounge();
  const tauriHost = useIsTauri();
  const [kernelLog, setKernelLog] = useState<string | null>(null);

  useEffect(() => {
    if (!tauriHost) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const paths = await invoke<{ kernel_log: string }>("get_runtime_paths");
        if (!cancelled) {
          setKernelLog(paths.kernel_log);
        }
      } catch {
        /* browser / mock */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauriHost]);

  const rows = semanticMap.projects.length
    ? semanticMap.projects.map((row) => {
        const fromList = deadSymbols.filter(
          (symbol) => !symbol.project_id || symbol.project_id === row.name,
        ).length;
        return {
          name: row.name || "unnamed",
          indexed: row.node_count > 0 || row.files > 0 ? 100 : 0,
          files: String(row.files),
          nodes: String(row.node_count),
          stale: 0,
          dead: fromList,
          sync: "live",
        };
      })
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
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => void indexWorkspace()}
            disabled={indexing}
            data-qa="health-reindex"
            className="min-h-8 rounded border border-outline-variant px-2 py-1 font-body text-meta font-medium text-primary hover:bg-primary-container/20 disabled:opacity-60"
          >
            {indexing ? "Scanning..." : dsStrings.reindex}
          </button>
          <span className="font-mono text-meta text-outline">memory_bridge · live</span>
        </div>
      </div>
      {kernelLog ? (
        <div
          className="shrink-0 border-b border-outline-variant/60 bg-surface-container-lowest/40 px-2.5 py-1.5 font-mono text-meta text-outline"
          data-qa="health-kernel-log"
        >
          <span className="font-semibold text-on-surface-variant">{t("diagnosticsTitle")}: </span>
          {t("diagnosticsKernelLog", { path: kernelLog })}
          <span className="mt-0.5 block">{t("diagnosticsHint")}</span>
        </div>
      ) : null}
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
                  <Link
                    href={`/health?tab=dead&project=${encodeURIComponent(repo.name)}`}
                    data-qa="health-dead-drilldown"
                    data-qa-dead-count={repo.dead}
                    className="font-medium text-error hover:underline"
                  >
                    {dsStrings.deadDrillDown(repo.dead)}
                  </Link>
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
  const { t } = useTranslation("quotas");
  const { quotas, quotaError, amberAlert, amberTools } = useLounge();
  const displayQuotas = useMemo(() => mergeClaudeQuotaRows(quotas), [quotas]);
  const critical = useMemo(() => selectCriticalQuotas(displayQuotas, 3), [displayQuotas]);

  return (
    <div className="space-y-2 p-2.5" data-testid="quota-mini-card">
      {amberAlert ? (
        <div className="rounded border border-error-container bg-error-container/20 px-2.5 py-1.5 font-body text-meta text-error-dim">
          {t("amberAlert", { tools: amberTools.join(", ") || t("amberAlertDefault") })}
        </div>
      ) : null}
      {critical.length === 0 ? (
        <div className="rounded border border-outline-variant/40 bg-surface-container-high/40 px-2.5 py-4 text-center font-body text-body text-outline">
          {quotaError ?? t("miniEmpty")}
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
          {t("miniAllList")}
        </Link>
      </div>
    </div>
  );
}

export function QuotaPanel() {
  const { t } = useTranslation("quotas");
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
            {t("title")}
          </h2>
          {amberAlert ? (
            <span className="rounded border border-error-container bg-error-container/20 px-1.5 py-0.5 font-mono text-meta font-semibold text-error-dim uppercase">
              {t("amberAlert", { tools: amberTools.join(", ") || t("amberAlertDefault") })}
            </span>
          ) : (
            <span className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-meta text-on-surface-variant">
              {t("probes")}
            </span>
          )}
        </div>
        <div className="flex flex-wrap items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-mono text-meta">
          {(["all", "subscription", "api", "plugin", "local"] as const).map((key) => {
            const label =
              key === "all"
                ? t("filterAll")
                : key === "subscription"
                  ? t("filterSubscription")
                  : key === "api"
                    ? t("filterApi")
                    : key === "plugin"
                      ? t("filterPlugin")
                      : t("filterLocal");
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
                {label}
              </button>
            );
          })}
        </div>
      </div>
      <div className="min-h-0 w-full flex-1 overflow-x-auto overflow-y-auto">
        <table className="w-full min-w-0 border-collapse text-left font-mono text-body table-fixed md:table-auto">
          <thead>
            <tr className="select-none border-b border-outline-variant bg-surface-container-low/80 text-meta text-outline uppercase">
              <th className="px-2.5 py-1.5 font-medium">{t("colTool")}</th>
              <th className="w-14 px-2 py-1.5 font-medium">{t("colKind")}</th>
              <th className="hidden w-16 px-2 py-1.5 font-medium sm:table-cell">{t("colUnit")}</th>
              <th className="px-2 py-1.5 font-medium">{t("colUsedLimit")}</th>
              <th className="hidden px-2 py-1.5 text-right font-medium md:table-cell">{t("colRemainingHosts")}</th>
              <th className="hidden px-2 py-1.5 text-right font-medium lg:table-cell">{t("colReset")}</th>
              <th className="w-40 min-w-[10rem] px-2.5 py-1.5 text-right font-medium">{t("colState")}</th>
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
                <td className="w-40 min-w-[10rem] max-w-[14rem] px-2.5 py-1.5 text-right">
                  <span
                    title={row.label}
                    className={`inline-flex max-w-full items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-meta ${quotaToneClass(row.tone)}`}
                  >
                    {row.tone === "live" ? <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-secondary" /> : null}
                    <span className="min-w-0 truncate">{row.label}</span>
                  </span>
                </td>
              </tr>
            ))}
            {rows.length === 0 ? (
              <tr>
                <td colSpan={7} className="px-2.5 py-6 text-center font-mono text-body text-outline">
                  {quotaError ?? t("emptyTools")}
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-1.5 font-mono text-meta text-outline">
        <span>{t("footerSources")}</span>
        <div className="flex items-center gap-3">
          <Pager page={safePage} pages={pages} total={rows.length} onPage={setPage} />
          <div className={`flex items-center gap-1.5 ${amberAlert ? "text-error-dim" : "text-outline"}`}>
            <span className={`h-1.5 w-1.5 rounded-full ${amberAlert ? "animate-pulse bg-error" : "bg-error"}`} />
            <span>{amberAlert ? t("amberAlertShort") : t("toolsNearCap", { count: nearCap })}</span>
          </div>
        </div>
      </div>
    </section>
  );
}

const QUOTA_ACTION_IDS: QuotaExhaustedAction[] = ["stop", "ask_then_local", "ask_then_abort"];

type RuntimePaths = {
  data_root: string;
  kernel_log: string;
  lmr_log: string;
  nats_log: string;
  lmr_dir: string;
  lmr_binary: string;
};

function DiagnosticsSettingsPanel() {
  const { t } = useTranslation("settings");
  const tauriHost = useIsTauri();
  const [paths, setPaths] = useState<RuntimePaths | null>(null);

  useEffect(() => {
    if (!tauriHost) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const next = await invoke<RuntimePaths>("get_runtime_paths");
        if (!cancelled) {
          setPaths(next);
        }
      } catch {
        if (!cancelled) {
          setPaths(null);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tauriHost]);

  return (
    <div className="rounded-lg border border-outline-variant bg-surface-container" data-qa="diagnostics-panel">
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("diagnosticsTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("diagnosticsDesc")}
        </p>
      </div>
      <div className="space-y-2 p-3 font-mono text-meta text-on-surface-variant">
        {!tauriHost ? (
          <p>{t("diagnosticsBrowser")}</p>
        ) : !paths ? (
          <p>{t("diagnosticsLoading")}</p>
        ) : (
          <dl className="space-y-2">
            <div>
              <dt className="text-outline">{t("diagnosticsDataRoot")}</dt>
              <dd className="break-all text-on-surface" data-qa="diagnostics-data-root">
                {paths.data_root}
              </dd>
            </div>
            <div>
              <dt className="text-outline">{t("diagnosticsKernelLog")}</dt>
              <dd className="break-all text-on-surface" data-qa="diagnostics-kernel-log">
                {paths.kernel_log}
              </dd>
            </div>
            <div>
              <dt className="text-outline">{t("diagnosticsLmrLog")}</dt>
              <dd className="break-all text-on-surface">{paths.lmr_log}</dd>
            </div>
            <div>
              <dt className="text-outline">{t("diagnosticsNatsLog")}</dt>
              <dd className="break-all text-on-surface">{paths.nats_log}</dd>
            </div>
            <div>
              <dt className="text-outline">{t("diagnosticsLmrBinary")}</dt>
              <dd className="break-all text-on-surface">{paths.lmr_binary}</dd>
            </div>
          </dl>
        )}
      </div>
    </div>
  );
}

export function SettingsPanel() {
  const { policy, savePolicy, model } = useLounge();
  const { scale, setScale, scales } = useUiScale();
  const { t } = useTranslation("settings");
  const { t: tq } = useTranslation("quotas");
  const { t: tc } = useTranslation("common");
  const { t: ts } = useTranslation("shell");

  const scaleLabel = (value: UiScale) => `${Math.round(value * 100)}%`;
  const quotaActions = QUOTA_ACTION_IDS.map((id) => ({
    id,
    title:
      id === "stop"
        ? tq("actionStop")
        : id === "ask_then_local"
          ? tq("actionAskLocal")
          : tq("actionAskAbort"),
    hint:
      id === "stop"
        ? tq("actionStopHint")
        : id === "ask_then_local"
          ? tq("actionAskLocalHint")
          : tq("actionAskAbortHint"),
  }));

  return (
    <section data-qa="panel" className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-auto">
      <div className="w-full space-y-3">
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
          <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">{t("routingPolicy")}</h2>
          <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
            {t("routingDesc")}
          </p>
        </div>
        <div className="space-y-3 p-3">
          <label
            className="flex items-center justify-between rounded border border-outline-variant bg-surface-container-high px-3 py-2 font-body text-body"
            title="Always required — cannot be disabled (security)"
          >
            <span>{t("agentSwitchLocked")}</span>
            <input
              type="checkbox"
              checked
              disabled
              title="Always required — cannot be disabled (security)"
              className="accent-primary"
            />
          </label>
          <div className="grid gap-2 md:grid-cols-3">
            {quotaActions.map((action) => {
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
              {t("localFallbackAgent")}
              <input
                value={policy.local_fallback_agent}
                onChange={(event) => void savePolicy({ ...policy, local_fallback_agent: event.target.value })}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-mono text-body text-on-surface"
              />
            </label>
            <label className="space-y-1 font-body text-meta text-on-surface-variant">
              {t("localFallbackModel")}
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
                  <th className="px-2.5 py-1.5">{tc("agent")}</th>
                  <th className="px-2 py-1.5">{tc("trigger")}</th>
                  <th className="px-2.5 py-1.5 text-right">{tc("enabled")}</th>
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
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
          <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
            {ts("language")}
          </h2>
          <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
            {ts("languageLabel")}
          </p>
        </div>
        <div className="p-3">
          <LocaleSwitch />
        </div>
      </div>
      <EditorSettingsPanel />
      <ExperienceGovernanceSettingsPanel />
      <ApprovalSoundSettingsPanel />
      <DestructiveGateSettingsPanel />
      <GraphUiSettings />
      <DiagnosticsSettingsPanel />
      <div className="rounded-lg border border-outline-variant bg-surface-container">
        <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
          <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
            {t("uiScale")}
          </h2>
          <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
            {t("uiScaleDesc")}
          </p>
        </div>
        <div className="flex flex-wrap gap-2 p-3" role="radiogroup" aria-label={t("uiScaleLabel")}>
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
            <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">{t("connectedTools")}</h2>
            <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
              Claude Desktop, Cursor, Antigravity, LMR, Ollama → connected_tools
            </p>
          </div>
          <Link
            href="/onboarding"
            className="min-h-8 rounded bg-primary-container px-2.5 py-1.5 font-body text-body font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed"
          >
            {t("rescan")}
          </Link>
        </div>
      </div>
      </div>
    </section>
  );
}

type FleetWorkerRow = {
  id: string;
  label: string;
  status: string;
  endpoint: string;
  detail: string;
  heartbeat: string;
  pid: string;
  uptime: string;
  restarts: string;
  tone: "ok" | "warn" | "down";
};

const FLEET_STATUS_KEYS: Record<string, string> = {
  ok: "statusOk",
  ready: "statusReady",
  "restart-limit": "statusRestartLimit",
  down: "statusDown",
  live: "statusLive",
  stale: "statusStale",
  supervised: "statusSupervised",
  up: "statusUp",
  max: "statusMax",
  online: "statusOnline",
  offline: "statusOffline",
  listening: "statusListening",
  missing: "statusMissing",
  "not-installed": "statusNotInstalled",
};

function translateFleetToken(t: (key: string) => string, token: string): string {
  const key = FLEET_STATUS_KEYS[token];
  return key ? t(key) : token;
}

export function FleetPanel() {
  const { t } = useTranslation("fleet");
  const { report, model, decisionGate, layaEngine } = useLounge();
  const copyEndpoint = (endpoint: string) => {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(endpoint);
    }
  };
  const engineLabel = formatLayaEngineFleetStatus(layaEngine);
  const downloadPct = layaEnginePercentage(layaEngine);
  const gateHint =
    layaEngine?.phase === "downloading" && downloadPct != null
      ? `${downloadPct}%`
      : decisionGate?.phase === "ready"
        ? decisionGate.device || t("decisionGate")
        : t("decisionGateOff");
  const [natsWorkers, setNatsWorkers] = useState<FleetWorkerRow[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>("lounge-kernel");

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
                endpoint: row.endpoint || t("endpointNats"),
                detail: row.payload?.detail || t("detailNatsWorker"),
                heartbeat: online ? "live" : "stale",
                pid: "—",
                uptime: online ? "up" : "—",
                restarts: "—",
                tone: online ? "ok" : ("down" as const),
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
  }, [t]);

  const layaTone: FleetWorkerRow["tone"] =
    layaEngine?.phase === "failed"
      ? "down"
      : layaEngine?.phase === "downloading"
        ? "warn"
        : "ok";
  const workers: FleetWorkerRow[] = [
    {
      id: "lounge-kernel",
      label: "lounge-kernel",
      ...fleetHealthFields(report?.ollama, "http://127.0.0.1:18790", model || "LMR"),
      detail:
        report?.ollama.error ||
        report?.ollama.detail ||
        (report?.ollama.running ? model || t("lmrReady") : t("lmrDown")),
    },
    {
      id: "nats-hub",
      label: "nats-hub",
      ...fleetHealthFields(report?.nats, "nats://127.0.0.1:4222", "lounge.>"),
      status: report?.nats.running ? "listening" : "down",
      detail: report?.nats.error || report?.nats.detail || t("natsListening"),
    },
    {
      id: "memory-bridge",
      label: "memory-bridge",
      ...fleetHealthFields(report?.memory, "http://127.0.0.1:7432", t("cbmMissing")),
      status: report?.memory.running ? "ready" : "missing",
    },
    {
      id: "openjev-laya",
      label: "openjev-laya",
      status: engineLabel,
      endpoint: layaEngine?.path || t("localWeights"),
      detail: gateHint,
      heartbeat: layaEngine?.phase === "ready" ? "live" : "—",
      pid: "—",
      uptime: layaEngine?.phase === "ready" ? "up" : "—",
      restarts: "—",
      tone: layaTone,
    },
    ...natsWorkers,
  ];
  const selected = workers.find((row) => row.id === selectedId) ?? workers[0] ?? null;

  return (
    <section
      data-qa="panel"
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="shrink-0 border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label uppercase">{t("title")}</h2>
      </div>
      <div className="flex min-h-0 w-full flex-1 flex-col overflow-hidden lg:flex-row">
        <div className="min-h-0 min-w-0 flex-1 overflow-auto">
          <table className="w-full table-fixed border-collapse text-left font-mono text-body">
            <thead>
              <tr className="sticky top-0 border-b border-outline-variant bg-surface-container-low/95 text-meta text-outline uppercase">
                <th className="w-[22%] px-3 py-2 font-medium">{t("colWorker")}</th>
                <th className="w-[12%] px-2 py-2 font-medium">{t("colStatus")}</th>
                <th className="hidden w-[12%] px-2 py-2 font-medium sm:table-cell">{t("colHeartbeat")}</th>
                <th className="hidden w-[10%] px-2 py-2 font-medium md:table-cell">{t("colPid")}</th>
                <th className="hidden w-[10%] px-2 py-2 font-medium md:table-cell">{t("colUptime")}</th>
                <th className="hidden w-[10%] px-2 py-2 font-medium lg:table-cell">{t("colRestarts")}</th>
                <th className="px-3 py-2 font-medium">{t("colEndpoint")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-outline-variant/40">
              {workers.map((row) => {
                const active = selected?.id === row.id;
                return (
                  <tr
                    key={row.id}
                    className={`cursor-pointer ${active ? "bg-surface-container-highest/70" : "hover:bg-surface-container-high/50"}`}
                    onClick={() => setSelectedId(row.id)}
                  >
                    <td className="truncate px-3 py-2.5 font-medium text-on-surface" title={row.label}>
                      {row.label}
                    </td>
                    <td
                      className={`px-2 py-2.5 ${
                        row.tone === "down"
                          ? "text-error"
                          : row.tone === "warn"
                            ? "text-on-surface-variant"
                            : "text-secondary"
                      }`}
                    >
                      {translateFleetToken(t, row.status)}
                    </td>
                    <td className="hidden px-2 py-2.5 text-on-surface-variant sm:table-cell">
                      {translateFleetToken(t, row.heartbeat)}
                    </td>
                    <td className="hidden px-2 py-2.5 text-on-surface-variant md:table-cell">
                      {translateFleetToken(t, row.pid)}
                    </td>
                    <td className="hidden px-2 py-2.5 text-on-surface-variant md:table-cell">
                      {translateFleetToken(t, row.uptime)}
                    </td>
                    <td className="hidden px-2 py-2.5 text-on-surface-variant lg:table-cell">
                      {translateFleetToken(t, row.restarts)}
                    </td>
                    <td className="px-3 py-2.5 text-on-surface-variant">
                      <button
                        type="button"
                        title={row.endpoint}
                        className="block w-full truncate text-left hover:underline"
                        onClick={(event) => {
                          event.stopPropagation();
                          copyEndpoint(row.endpoint);
                        }}
                      >
                        {row.endpoint}
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        <aside className="flex min-h-[10rem] w-full shrink-0 flex-col border-t border-outline-variant bg-surface-container-low/40 lg:w-72 lg:border-t-0 lg:border-l">
          <div className="border-b border-outline-variant px-3 py-2 font-body text-meta font-semibold tracking-label text-on-surface uppercase">
            {t("detailTitle")}
          </div>
          {selected ? (
            <dl className="min-h-0 flex-1 space-y-2 overflow-auto px-3 py-3 font-mono text-body">
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailName")}</dt>
                <dd className="text-on-surface">{selected.label}</dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailStatus")}</dt>
                <dd className={selected.tone === "down" ? "text-error" : "text-secondary"}>
                  {translateFleetToken(t, selected.status)}
                </dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailHeartbeat")}</dt>
                <dd className="text-on-surface-variant">{translateFleetToken(t, selected.heartbeat)}</dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailPid")}</dt>
                <dd className="text-on-surface-variant">{translateFleetToken(t, selected.pid)}</dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailUptime")}</dt>
                <dd className="text-on-surface-variant">{translateFleetToken(t, selected.uptime)}</dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailRestarts")}</dt>
                <dd className="text-on-surface-variant">{translateFleetToken(t, selected.restarts)}</dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailEndpoint")}</dt>
                <dd className="break-all text-on-surface-variant">{selected.endpoint}</dd>
              </div>
              <div>
                <dt className="text-meta text-outline uppercase">{t("detailDetail")}</dt>
                <dd className="break-words text-on-surface-variant">{selected.detail}</dd>
              </div>
            </dl>
          ) : (
            <p className="px-3 py-4 font-body text-body text-on-surface-variant">{t("noWorkerSelected")}</p>
          )}
          <div className="mt-auto space-y-1 border-t border-outline-variant px-3 py-2 font-body text-meta text-outline">
            <p>{t("footerHeartbeat")}</p>
            <p className="font-mono">lounge.workers.heartbeat</p>
          </div>
        </aside>
      </div>
      <div className="flex shrink-0 items-center justify-between gap-2 border-t border-outline-variant bg-surface-container-low px-3 py-2 font-body text-meta text-outline">
        <span className="font-body">{t("workersRegistered", { count: workers.length })}</span>
        <span className="font-mono">{t("footerSupervisor")}</span>
      </div>
    </section>
  );
}

export function TelemetryPanel() {
  const { t } = useTranslation("telemetry");
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
  const [saveToast, setSaveToast] = useState<string | null>(null);
  const [savingMd, setSavingMd] = useState(false);
  const tauriHost = useIsTauri();

  useEffect(() => {
    if (!saveToast) return;
    const id = window.setTimeout(() => setSaveToast(null), 5000);
    return () => window.clearTimeout(id);
  }, [saveToast]);

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
          <div className="text-body tracking-wider text-on-surface-variant uppercase">
            {t("layaDecision")}
          </div>
          <div
            key={latencyLive ? formatLatencyMs(latencyMs) : "idle"}
            className="kpi-tick mt-2 tnum text-2xl font-bold text-on-surface"
          >
            {latencyLive ? formatLatencyMs(latencyMs) : "—"}
          </div>
          <div className="text-on-surface-variant">{formatLayaDecision(latencyLive ? latencyMs : null)}</div>
          <div className="mt-3 min-h-0 flex-1 overflow-auto">
            {latencyLive ? (
              <LatencySparkline values={decisionLatencyHistory} />
            ) : (
              <span className="text-body text-on-surface-variant">
                {decisionGate?.phase === "ready" ? t("idleNoInfer") : t("gateCold")}
              </span>
            )}
          </div>
          <div className="mt-auto pt-4 text-body text-on-surface-variant">
            lounge.telemetry.decision · {decisionTelemetry?.device ?? decisionGate?.device ?? "—"}
          </div>
        </div>
        <div className="flex min-h-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container p-3 font-mono text-body">
          <div className="text-body tracking-wider text-on-surface-variant uppercase">{t("msgPerMin")}</div>
          <div
            key={decisionMsgPerMin}
            className="kpi-tick mt-2 tnum text-2xl font-bold text-on-surface"
          >
            {decisionMsgPerMin}
          </div>
          <div className="text-on-surface-variant">{msgLive ? t("nats60sLive") : t("nats60sIdle")}</div>
          <div className="mt-auto pt-4 text-body text-on-surface-variant">
            {t("natsBufferLine", {
              events: events.length,
              subscriptions: subscription.length,
              plugins: plugins.length,
            })}
          </div>
        </div>
      </div>

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container">
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-outline-variant bg-surface-container-low px-3 py-2">
          <h2 className="font-body text-panel font-semibold tracking-label uppercase">
            {t("efficiencyTitle")}
          </h2>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <label className="flex items-center gap-1.5 font-mono text-body text-on-surface-variant">
              <input
                type="checkbox"
                checked={weekly}
                onChange={(e) => setWeekly(e.target.checked)}
                className="accent-primary"
              />
              {t("weekly")}
            </label>
            <select
              value={projectId}
              onChange={(e) => setProjectId(e.target.value)}
              className="max-w-[10rem] truncate rounded border border-outline-variant bg-surface px-1.5 py-1 font-mono text-body text-on-surface"
              aria-label={t("projectFilter")}
            >
              <option value="">{t("allProjects")}</option>
              {projectOptions.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>
            <button
              type="button"
              disabled={!report || savingMd}
              onClick={() => {
                if (!report) return;
                const stamp = report.generatedAt.slice(0, 10);
                setSavingMd(true);
                void saveMarkdownReport(`agent-efficiency-${stamp}.md`, report.markdown)
                  .then((result) => {
                    if (result.ok) {
                      setSaveToast(
                        result.mode === "tauri"
                          ? t("savedTauri", { path: result.path })
                          : t("savedDownload", { path: result.path }),
                      );
                    } else if (!result.cancelled) {
                      setSaveToast(t("saveFailed", { error: result.error }));
                    }
                  })
                  .finally(() => setSavingMd(false));
              }}
              className="rounded border border-outline-variant bg-surface-container-high px-2 py-1 font-mono text-meta font-bold tracking-wider text-on-surface uppercase enabled:hover:bg-surface-container disabled:opacity-40"
            >
              {savingMd ? t("saving") : t("downloadMarkdown")}
            </button>
          </div>
          {saveToast ? (
            <p
              className="w-full px-3 pb-2 font-mono text-meta text-secondary"
              role="status"
              data-qa="markdown-save-toast"
            >
              {saveToast}
            </p>
          ) : null}
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-3 font-mono text-body">
          {loading && !report ? (
            <p className="text-on-surface-variant">{t("loadingReport")}</p>
          ) : null}
          {error ? (
            <p className="mb-2 text-meta text-error">{t("tauriReportError", { error })}</p>
          ) : null}
          {report ? (
            <div className="space-y-4">
              <p className="text-meta text-outline">
                {report.scopeLabel}
                {tauriHost ? "" : t("browserSuffix")}
                {loading ? t("refreshingSuffix") : ""}
              </p>
              <div className="grid gap-2 sm:grid-cols-3">
                <div className="rounded border border-outline-variant/60 bg-surface-container-low px-2.5 py-2">
                  <div className="text-meta tracking-wider text-outline uppercase">
                    {t("totalFailure")}
                  </div>
                  <div className="tnum mt-1 text-xl font-bold text-on-surface">
                    {report.totalFailures}
                  </div>
                  <div className="text-meta text-on-surface-variant">{t("failureByAgentHint")}</div>
                </div>
                <div className="rounded border border-outline-variant/60 bg-surface-container-low px-2.5 py-2">
                  <div className="text-meta tracking-wider text-outline uppercase">
                    {t("crossProject")}
                  </div>
                  <div className="tnum mt-1 text-xl font-bold text-on-surface">
                    {report.crossProjectExperienceHits}
                  </div>
                  <div className="text-meta text-on-surface-variant">
                    {t("whisperEvents", { count: report.crossProjectWhisperEvents })}
                  </div>
                </div>
                <div className="rounded border border-outline-variant/60 bg-surface-container-low px-2.5 py-2">
                  <div className="text-meta tracking-wider text-outline uppercase">
                    {t("deadCleanup")}
                  </div>
                  <div className="tnum mt-1 text-xl font-bold text-on-surface">{rateLabel}</div>
                  <div className="text-meta text-on-surface-variant">
                    {t("deadRemaining", { remaining: report.dead.remaining })}
                    {report.dead.cleaned != null
                      ? t("deadCleaned", { cleaned: report.dead.cleaned })
                      : ""}
                  </div>
                </div>
              </div>

              <div>
                <h3 className="mb-1.5 text-meta font-bold tracking-wider text-outline uppercase">
                  {t("failuresByAgent")}
                </h3>
                {report.failuresByAgent.length === 0 ? (
                  <p className="text-on-surface-variant">{t("noRecords")}</p>
                ) : (
                  <table className="w-full text-left">
                    <thead>
                      <tr className="text-meta text-outline">
                        <th className="py-1 font-normal">{t("colAgent")}</th>
                        <th className="py-1 text-right font-normal">{t("colCount")}</th>
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
                <div className="mb-1 font-bold tracking-wider text-outline uppercase">{t("source")}</div>
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
          {t("footerEfficiency", { scope: report ? report.scopeLabel : t("footerWaiting") })}
        </span>
        <span className="font-mono text-meta text-outline">
          {t("footerBuffer", { events: events.length, dead: deadSymbols.length })}
        </span>
      </div>
    </section>
  );
}
