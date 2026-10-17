"use client";

import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { PanelGrid } from "@/components/design-system";
import { AgentStatusBadge } from "@/components/health/agent-status-badge";
import {
  maskAgentEntryForUi,
  maskHomePathForUi,
  statusLabelKey,
} from "@/lib/health-agents";
import {
  fetchLoungeHealth,
  isTauri,
  probeAgentMcpUrl,
  type AgentHealthEntry,
  type AgentMcpProbeResult,
  type AgentMcpStatus,
  type LoungeHealthReport,
} from "@/lib/lounge";

type ListItem =
  | { kind: "agent"; id: string; entry: AgentHealthEntry }
  | { kind: "title"; id: "title" }
  | { kind: "cbm"; id: "cbm" }
  | { kind: "platform"; id: "platform" };

type LoadState =
  | { phase: "browser" }
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "empty"; report: LoungeHealthReport }
  | { phase: "ready"; report: LoungeHealthReport };

function agentItemId(entry: AgentHealthEntry, index: number): string {
  return `agent:${entry.client}:${entry.config_path ?? ""}:${index}`;
}

export function AgentsHealthPanel() {
  const { t } = useTranslation("health");
  const [state, setState] = useState<LoadState>(() =>
    isTauri() ? { phase: "loading" } : { phase: "browser" },
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [probe, setProbe] = useState<AgentMcpProbeResult | null>(null);
  const [probing, setProbing] = useState(false);
  const listLabelId = useId();

  const load = useCallback(async () => {
    if (!isTauri()) {
      setState({ phase: "browser" });
      return;
    }
    setState({ phase: "loading" });
    setProbe(null);
    try {
      const report = await fetchLoungeHealth();
      const hasAgents = report.agents.length > 0;
      setState(hasAgents ? { phase: "ready", report } : { phase: "empty", report });
    } catch (error) {
      setState({
        phase: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  useEffect(() => {
    // Defer so the initial setState is not synchronous inside the effect body
    // (react-hooks/set-state-in-effect).
    const timer = window.setTimeout(() => {
      void load();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [load]);

  const report =
    state.phase === "ready" || state.phase === "empty" ? state.report : null;

  const items: ListItem[] = useMemo(() => {
    if (!report) return [];
    const agents = report.agents.map((entry, index) => {
      const masked = maskAgentEntryForUi(entry);
      return {
        kind: "agent" as const,
        id: agentItemId(masked, index),
        entry: masked,
      };
    });
    return [
      ...agents,
      { kind: "title" as const, id: "title" },
      { kind: "cbm" as const, id: "cbm" },
      { kind: "platform" as const, id: "platform" },
    ];
  }, [report]);

  const selected = useMemo(() => {
    if (items.length === 0) return null;
    return items.find((item) => item.id === selectedId) ?? items[0] ?? null;
  }, [items, selectedId]);

  const selectItem = useCallback((id: string) => {
    setSelectedId(id);
    setProbe(null);
  }, []);

  const onProbe = useCallback(async () => {
    if (!selected || selected.kind !== "agent") return;
    if (selected.entry.status !== "unverified") return;
    const pathId = selected.entry.config_path?.trim();
    if (!pathId) return;
    setProbing(true);
    try {
      const result = await probeAgentMcpUrl(selected.entry.client, pathId);
      setProbe(result);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setProbe({
        outcome: /config|missing|not found/i.test(message)
          ? "config_missing"
          : "unreachable",
        elapsed_ms: 0,
      });
    } finally {
      setProbing(false);
    }
  }, [selected]);

  const moveSelection = useCallback(
    (delta: number) => {
      if (items.length === 0) return;
      const current = selected ? items.findIndex((i) => i.id === selected.id) : 0;
      const next = Math.max(0, Math.min(items.length - 1, current + delta));
      selectItem(items[next]!.id);
    },
    [items, selected, selectItem],
  );

  if (state.phase === "browser") {
    return (
      <section
        data-qa="panel"
        data-qa-agents-health="browser"
        className="flex h-full min-h-0 w-full flex-col items-center justify-center gap-2 rounded-lg border border-outline-variant bg-surface-container p-6 text-center"
      >
        <p className="max-w-md font-body text-body text-on-surface-variant">
          {t("agents.browserMode")}
        </p>
      </section>
    );
  }

  if (state.phase === "loading") {
    return (
      <section
        data-qa="panel"
        data-qa-agents-health="loading"
        className="flex h-full min-h-0 w-full flex-col gap-3 rounded-lg border border-outline-variant bg-surface-container p-4"
        aria-busy="true"
        aria-label={t("agents.loading")}
      >
        <div className="h-4 w-1/3 animate-pulse rounded bg-surface-container-high" />
        <div className="h-24 animate-pulse rounded bg-surface-container-high" />
        <div className="h-24 animate-pulse rounded bg-surface-container-high" />
        <div className="h-24 animate-pulse rounded bg-surface-container-high" />
      </section>
    );
  }

  if (state.phase === "error") {
    return (
      <section
        data-qa="panel"
        data-qa-agents-health="error"
        className="flex h-full min-h-0 w-full flex-col items-center justify-center gap-3 rounded-lg border border-outline-variant bg-surface-container p-6 text-center"
      >
        <p className="max-w-lg font-body text-body text-error">
          {t("agents.error", { message: state.message })}
        </p>
        <button
          type="button"
          data-qa="agents-health-retry"
          onClick={() => void load()}
          className="min-h-8 rounded border border-primary/40 px-3 py-1 font-body text-body text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
        >
          {t("agents.retry")}
        </button>
      </section>
    );
  }

  if (state.phase === "empty") {
    return (
      <section
        data-qa-agents-health="empty"
        className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden"
      >
        <PanelGrid
          className="min-h-[28rem] flex-1"
          list={
            <EmptyList
              message={t("agents.empty")}
              title={t("agents.listTitle")}
              systemItems={items}
              selectedId={selected?.id ?? null}
              onSelect={selectItem}
              t={t}
            />
          }
          detail={
            <DetailPane
              item={selected}
              report={state.report}
              probe={probe}
              probing={probing}
              onProbe={() => void onProbe()}
              t={t}
            />
          }
          inspector={
            <InspectorPane item={selected} report={state.report} t={t} />
          }
        />
      </section>
    );
  }

  // ready — PanelGrid slots own data-qa="panel" (avoid double-counting for L1–L6).
  return (
    <section
      data-qa-agents-health="ready"
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden"
    >
      <PanelGrid
        className="min-h-[28rem] flex-1"
        list={
          <div className="flex h-full min-h-0 flex-col">
            <div className="shrink-0 border-b border-outline-variant bg-surface-container-low px-3 py-2">
              <h2
                id={listLabelId}
                className="font-body text-panel font-semibold tracking-label uppercase"
              >
                {t("agents.listTitle")}
              </h2>
              <p className="font-mono text-meta text-outline">
                {t("agents.checkedAt")}: {report!.checked_at}
              </p>
              <p className="font-body text-meta text-on-surface-variant">
                {t("agents.sectionAgents")} · {t("agents.sectionTitle")} ·{" "}
                {t("agents.sectionCbm")} · {t("agents.sectionPlatform")}
              </p>
            </div>
            <div
              role="group"
              tabIndex={0}
              aria-labelledby={listLabelId}
              aria-label={t("agents.listTitle")}
              data-qa="agents-health-list"
              className="min-h-0 flex-1 overflow-auto font-mono text-body focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-primary"
              onKeyDown={(event) => {
                if (event.key === "ArrowDown") {
                  event.preventDefault();
                  moveSelection(1);
                } else if (event.key === "ArrowUp") {
                  event.preventDefault();
                  moveSelection(-1);
                } else if (event.key === "Enter" && selected) {
                  event.preventDefault();
                  selectItem(selected.id);
                }
              }}
            >
              {items.map((item) => (
                <ListRow
                  key={item.id}
                  item={item}
                  active={selected?.id === item.id}
                  onSelect={() => selectItem(item.id)}
                  t={t}
                />
              ))}
              {/* Depth filler: honest help copy so tall panels satisfy L2 interior fill. */}
              <div className="space-y-2 border-t border-outline-variant/40 px-3 py-4">
                <p className="font-body text-meta text-on-surface-variant">{t("agents.probeHint")}</p>
                <p className="font-body text-meta text-outline">{t("agents.empty")}</p>
                <p className="font-body text-meta text-outline">
                  {t("agents.fieldConfigPath")} · {t("agents.fieldCommand")} ·{" "}
                  {t("agents.fieldExpectedVersion")}
                </p>
                <p className="font-body text-meta text-outline">
                  schema {report!.schema_version} · {report!.agents.length} agents
                </p>
                <p className="font-body text-meta text-outline">{t("agents.selectEntry")}</p>
                <p className="font-body text-meta text-outline">{t("diagnosticsHint")}</p>
              </div>
            </div>
          </div>
        }
        detail={
          <DetailPane
            item={selected}
            report={report!}
            probe={probe}
            probing={probing}
            onProbe={() => void onProbe()}
            t={t}
          />
        }
        inspector={<InspectorPane item={selected} report={report!} t={t} />}
      />
    </section>
  );
}

function EmptyList({
  message,
  title,
  systemItems,
  selectedId,
  onSelect,
  t,
}: {
  message: string;
  title: string;
  systemItems: ListItem[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <h2 className="font-body text-panel font-semibold tracking-label uppercase">{title}</h2>
      </div>
      <p
        data-qa="agents-health-empty-message"
        className="shrink-0 border-b border-outline-variant/40 px-3 py-3 font-body text-body text-on-surface-variant"
      >
        {message}
      </p>
      <div role="group" data-qa="agents-health-list" className="min-h-0 flex-1 overflow-auto">
        {systemItems
          .filter((i) => i.kind !== "agent")
          .map((item) => (
            <ListRow
              key={item.id}
              item={item}
              active={(selectedId ?? systemItems.find((s) => s.kind !== "agent")?.id) === item.id}
              onSelect={() => onSelect(item.id)}
              t={t}
            />
          ))}
      </div>
    </div>
  );
}

function ListRow({
  item,
  active,
  onSelect,
  t,
}: {
  item: ListItem;
  active: boolean;
  onSelect: () => void;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  let title = "";
  let badge: ReactNode = null;
  if (item.kind === "agent") {
    title = item.entry.client;
    const status = item.entry.status as AgentMcpStatus;
    badge = (
      <AgentStatusBadge
        status={status}
        severity={item.entry.severity}
        label={t(statusLabelKey(status))}
      />
    );
  } else if (item.kind === "title") {
    title = t("agents.sectionTitle");
  } else if (item.kind === "cbm") {
    title = t("agents.sectionCbm");
  } else {
    title = t("agents.sectionPlatform");
  }

  return (
    <button
      type="button"
      aria-pressed={active}
      aria-current={active ? "true" : undefined}
      data-qa="agents-health-row"
      data-row-id={item.id}
      data-row-kind={item.kind}
      data-active={active ? "true" : "false"}
      onClick={onSelect}
      className={`flex w-full items-center justify-between gap-2 border-b border-outline-variant/40 px-3 py-2.5 text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-primary ${
        active
          ? "bg-primary-container/20 text-primary"
          : "text-on-surface hover:bg-surface-container-high"
      }`}
    >
      <span className="min-w-0 truncate font-mono text-body">{title}</span>
      {badge}
    </button>
  );
}

function DetailPane({
  item,
  report,
  probe,
  probing,
  onProbe,
  t,
}: {
  item: ListItem | null;
  report: LoungeHealthReport;
  probe: AgentMcpProbeResult | null;
  probing: boolean;
  onProbe: () => void;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  return (
    <div data-qa="agents-health-detail" className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <h2 className="font-body text-panel font-semibold tracking-label uppercase">
          {t("agents.detailTitle")}
        </h2>
        <p className="font-body text-meta text-on-surface-variant">{t("agents.probeHint")}</p>
      </div>
      <div className="min-h-0 flex-1 space-y-2 overflow-auto p-3 font-body text-body">
        {!item ? (
          <p className="text-on-surface-variant">{t("agents.selectEntry")}</p>
        ) : item.kind === "agent" ? (
          <AgentDetail
            entry={item.entry}
            probe={probe}
            probing={probing}
            onProbe={onProbe}
            t={t}
          />
        ) : item.kind === "title" ? (
          <TitleDetail report={report} t={t} />
        ) : item.kind === "cbm" ? (
          <CbmDetail report={report} t={t} />
        ) : (
          <PlatformDetail report={report} t={t} />
        )}
        <div className="space-y-2 border-t border-outline-variant/40 pt-3">
          <p className="font-body text-meta text-outline">{t("agents.fieldSeverity")}</p>
          <p className="font-body text-meta text-outline">{t("agents.fieldStatus")}</p>
          <p className="font-body text-meta text-outline">{t("diagnosticsHint")}</p>
          <p className="font-body text-meta text-outline">
            {t("agents.checkedAt")}: {report.checked_at}
          </p>
          <p className="font-body text-meta text-outline">
            {t("agents.yes")} / {t("agents.no")} · {t("agents.retry")}
          </p>
        </div>
      </div>
    </div>
  );
}

function AgentDetail({
  entry,
  probe,
  probing,
  onProbe,
  t,
}: {
  entry: AgentHealthEntry;
  probe: AgentMcpProbeResult | null;
  probing: boolean;
  onProbe: () => void;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  const status = entry.status as AgentMcpStatus;
  const showProbe = status === "unverified";
  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <AgentStatusBadge
          status={status}
          severity={entry.severity}
          label={t(statusLabelKey(status))}
        />
        <span className="font-mono text-meta text-outline">{entry.severity}</span>
      </div>
      <Field label={t("agents.fieldClient")} value={entry.client} />
      <Field
        label={t("agents.fieldConfigPath")}
        value={maskHomePathForUi(entry.config_path) || "—"}
      />
      <Field
        label={t("agents.fieldCommand")}
        value={maskHomePathForUi(entry.command) || "—"}
      />
      <Field
        label={t("agents.fieldConfiguredVersion")}
        value={entry.configured_version ?? "—"}
      />
      <Field
        label={t("agents.fieldExpectedVersion")}
        value={entry.expected_version ?? "—"}
      />
      <Field
        label={t("agents.fieldConfiguredSha")}
        value={entry.configured_sha256 ?? "—"}
      />
      <Field
        label={t("agents.fieldExpectedSha")}
        value={entry.expected_sha256 ?? "—"}
      />
      <Field
        label={t("agents.fieldDetail")}
        value={entry.detail?.trim() ? entry.detail : "—"}
      />
      {showProbe ? (
        <div className="space-y-2 border-t border-outline-variant/40 pt-3">
          <p className="font-body text-meta text-on-surface-variant">{t("agents.probeHint")}</p>
          <button
            type="button"
            data-qa="agents-probe-now"
            disabled={probing}
            onClick={onProbe}
            className="min-h-8 rounded border border-primary/40 bg-primary-container px-3 py-1 font-body text-body font-medium text-on-primary-container focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:opacity-60"
          >
            {probing ? t("agents.probeRunning") : t("agents.probeNow")}
          </button>
          {probe ? <ProbeNote probe={probe} t={t} /> : null}
        </div>
      ) : null}
    </>
  );
}

function ProbeNote({
  probe,
  t,
}: {
  probe: AgentMcpProbeResult;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  const ms = probe.elapsed_ms;
  let text = t("agents.probeUnreachable", { ms });
  if (probe.outcome === "reachable") {
    text = t("agents.probeReachable", { ms });
  } else if (probe.outcome === "not_probed_remote") {
    text = t("agents.probeNotProbedRemote");
  } else if (probe.outcome === "config_missing") {
    text = t("agents.probeConfigMissing");
  }
  return (
    <p data-qa="agents-probe-result" data-outcome={probe.outcome} className="font-body text-meta text-on-surface">
      {text}
    </p>
  );
}

function TitleDetail({
  report,
  t,
}: {
  report: LoungeHealthReport;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  const title = report.title;
  return (
    <>
      <Field label={t("agents.titleQueue")} value={String(title.queue_backlog)} />
      <Field label={t("agents.titleDb")} value={String(title.db_backlog)} />
      <Field label={t("agents.titleGiveUps")} value={String(title.give_ups)} />
      <Field
        label={t("agents.titleLmr")}
        value={title.lmr_reachable ? t("agents.yes") : t("agents.no")}
      />
      <Field
        label={t("agents.titleModel")}
        value={title.model_installed ? t("agents.yes") : t("agents.no")}
      />
      <Field label={t("agents.titleModelName")} value={title.model || "—"} />
      <Field
        label={t("agents.titleEndpoint")}
        value={title.lmr_endpoint?.trim() ? title.lmr_endpoint : "—"}
      />
    </>
  );
}

function CbmDetail({
  report,
  t,
}: {
  report: LoungeHealthReport;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  const cbm = report.cbm;
  return (
    <>
      <Field
        label={t("agents.cbmDaemon")}
        value={cbm.daemon_mode ? t("agents.yes") : t("agents.no")}
      />
      <Field label={t("agents.cbmPresence")} value={cbm.daemon_presence ?? "—"} />
      <Field
        label={t("agents.cbmPolluted")}
        value={cbm.polluted_18749 ? t("agents.yes") : t("agents.no")}
      />
      <Field
        label={t("agents.cbmConfig")}
        value={maskHomePathForUi(cbm.config_path) || "—"}
      />
      <Field
        label={t("agents.cbmUi")}
        value={
          cbm.ui_enabled == null
            ? "—"
            : cbm.ui_enabled
              ? t("agents.yes")
              : t("agents.no")
        }
      />
      <Field
        label={t("agents.cbmPort")}
        value={cbm.ui_port != null ? String(cbm.ui_port) : "—"}
      />
    </>
  );
}

function PlatformDetail({
  report,
  t,
}: {
  report: LoungeHealthReport;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  const p = report.platform;
  const distro = [p.distro_id, p.distro_id_like].filter(Boolean).join(" / ") || "—";
  return (
    <>
      <Field label={t("agents.platformOs")} value={p.os} />
      <Field label={t("agents.platformArch")} value={p.arch} />
      <Field label={t("agents.platformDistro")} value={distro} />
      <Field label={t("agents.platformPkg")} value={p.package_manager} />
    </>
  );
}

function InspectorPane({
  item,
  report,
  t,
}: {
  item: ListItem | null;
  report: LoungeHealthReport;
  t: (key: string, opts?: Record<string, unknown>) => string;
}) {
  const raw =
    item?.kind === "agent"
      ? maskAgentEntryForUi(item.entry)
      : item?.kind === "title"
        ? report.title
        : item?.kind === "cbm"
          ? {
              ...report.cbm,
              config_path: maskHomePathForUi(report.cbm.config_path),
            }
          : item?.kind === "platform"
            ? report.platform
            : null;

  return (
    <div data-qa="agents-health-inspector" className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <h2 className="font-body text-panel font-semibold tracking-label uppercase">
          {t("agents.inspectorTitle")}
        </h2>
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-auto p-3">
        <div>
          <div className="font-mono text-meta uppercase tracking-wider text-outline">
            {t("agents.rawFields")}
          </div>
          <pre
            data-qa="agents-health-raw"
            className="mt-1 overflow-auto whitespace-pre-wrap break-all rounded border border-outline-variant/40 bg-surface-container-low p-2 font-mono text-meta text-on-surface"
          >
            {raw ? JSON.stringify(raw, null, 2) : "—"}
          </pre>
        </div>
        <div>
          <div className="font-mono text-meta uppercase tracking-wider text-outline">
            {t("agents.actions")}
          </div>
          <p className="mt-1 font-body text-meta text-on-surface-variant">
            {item?.kind === "agent" && item.entry.status === "unverified"
              ? t("agents.probeHint")
              : "—"}
          </p>
          <p className="mt-2 font-body text-meta text-outline">{t("agents.empty")}</p>
          <p className="font-body text-meta text-outline">{t("diagnosticsHint")}</p>
          <p className="font-body text-meta text-outline">
            {t("agents.fieldConfiguredSha")} · {t("agents.fieldExpectedSha")}
          </p>
          <p className="font-body text-meta text-outline">
            {t("agents.checkedAt")}: {report.checked_at}
          </p>
        </div>
      </div>
    </div>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="font-mono text-meta uppercase tracking-wider text-outline">{label}</div>
      <div className="break-all font-mono text-body text-on-surface">{value}</div>
    </div>
  );
}
