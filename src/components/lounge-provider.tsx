"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  DEFAULT_POLICY,
  formatClock,
  indexBusMessage,
  isTauri,
  pickWorkspaceFolder,
  mockIndexSnapshot,
  MOCK_EVENTS,
  MOCK_EXPERIENCES,
  MOCK_QUOTAS,
  loungeMessageToEvent,
  AMBER_THRESHOLD,
  BUS_UI_EVENT,
  DECISION_GATE_EVENT,
  LAYA_ENGINE_EVENT,
  LATENCY_SPARK_CAP,
  MODEL_PULL_EVENT,
  QUOTA_UI_EVENT,
  SERVICE_UI_EVENT,
  parseDecisionTelemetry,
  parseLayaEngineStatus,
  parseSecurityAlert,
  parseContextWhisper,
  mergeWhisperedExperiences,
  pruneMsgWindow,
  recordMsgTick,
  type ApprovalRequest,
  type DeadSymbol,
  type DecisionGateStatus,
  type IndexNotice,
  type LayaEngineStatus,
  type IndexSnapshot,
  type LoungeExperience,
  type LoungeMessage,
  type LoungeTelemetry,
  type MsgTick,
  type NatsEvent,
  type ProjectSummary,
  type PullProgress,
  type QuotaState,
  type RoutingPolicy,
  type RoutingVote,
  type SemanticMap,
  type ServiceReport,
  type ToolQuota,
} from "@/lib/lounge";

const EVENT_CAP = 500;

type LoungeContextValue = {
  report: ServiceReport | null;
  kernel: string;
  model: string;
  setModel: (value: string) => void;
  models: string[];
  experiences: LoungeExperience[];
  /** Knowledge Vault'ta parlatılacak canlı fısıltı tecrübe id'leri. */
  whisperedExperienceIds: string[];
  events: NatsEvent[];
  quotas: ToolQuota[];
  amberAlert: boolean;
  amberTools: string[];
  projects: ProjectSummary[];
  lastIndex: IndexSnapshot | null;
  semanticMap: SemanticMap;
  deadSymbols: DeadSymbol[];
  query: string;
  setQuery: (value: string) => void;
  clock: string;
  indexing: boolean;
  indexNotice: IndexNotice | null;
  policy: RoutingPolicy;
  approval: ApprovalRequest | null;
  decisionGate: DecisionGateStatus | null;
  layaEngine: LayaEngineStatus | null;
  decisionTelemetry: LoungeTelemetry | null;
  decisionLatencyHistory: number[];
  decisionMsgPerMin: number;
  applyModel: (next?: string) => Promise<void>;
  enableLaya: () => Promise<void>;
  declineLaya: () => Promise<void>;
  indexWorkspace: () => Promise<void>;
  refresh: () => Promise<void>;
  savePolicy: (next: RoutingPolicy) => Promise<void>;
  resolveApproval: (vote: RoutingVote) => Promise<void>;
  ingestBusMessage: (message: LoungeMessage) => void;
  probeBus: () => Promise<void>;
};

const LoungeContext = createContext<LoungeContextValue | null>(null);

export function LoungeProvider({ children }: { children: ReactNode }) {
  const [report, setReport] = useState<ServiceReport | null>(null);
  // SSR ve ilk hydrate aynı olmalı; isTauri() useState initializer'da hydration bozar.
  const [kernel, setKernel] = useState("idle");
  const [model, setModel] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [experiences, setExperiences] = useState<LoungeExperience[]>(MOCK_EXPERIENCES);
  const [whisperedExperienceIds, setWhisperedExperienceIds] = useState<string[]>([]);
  const [events, setEvents] = useState<NatsEvent[]>(MOCK_EVENTS);
  const [quotas, setQuotas] = useState<ToolQuota[]>(MOCK_QUOTAS);
  const [amberAlert, setAmberAlert] = useState(
    MOCK_QUOTAS.some((row) => (row.percent ?? 0) >= AMBER_THRESHOLD),
  );
  const [amberTools, setAmberTools] = useState<string[]>(
    MOCK_QUOTAS.filter((row) => (row.percent ?? 0) >= AMBER_THRESHOLD).map((row) => row.id),
  );
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [lastIndex, setLastIndex] = useState<IndexSnapshot | null>(null);
  const [semanticMap, setSemanticMap] = useState<SemanticMap>({ projects: [] });
  const [deadSymbols, setDeadSymbols] = useState<DeadSymbol[]>([]);
  const [query, setQuery] = useState("");
  const [clock, setClock] = useState("--:--");
  const [indexing, setIndexing] = useState(false);
  const [indexNotice, setIndexNotice] = useState<IndexNotice | null>(null);
  const [policy, setPolicy] = useState<RoutingPolicy>(DEFAULT_POLICY);
  const [approval, setApproval] = useState<ApprovalRequest | null>(null);
  const [decisionGate, setDecisionGate] = useState<DecisionGateStatus | null>(null);
  const [layaEngine, setLayaEngine] = useState<LayaEngineStatus | null>(null);
  const [decisionTelemetry, setDecisionTelemetry] = useState<LoungeTelemetry | null>(null);
  const [decisionLatencyHistory, setDecisionLatencyHistory] = useState<number[]>([]);
  const [decisionMsgTimes, setDecisionMsgTimes] = useState<MsgTick[]>([]);
  const lastTelemetryId = useRef<string | null>(null);

  const refreshSemantic = useCallback(async () => {
    if (!isTauri()) {
      return;
    }
    try {
      const rows = await invoke<LoungeExperience[]>("list_experiences", { limit: 12 });
      setExperiences(rows);
    } catch {
      /* SQLite henüz boş olabilir */
    }
    try {
      const map = await invoke<SemanticMap>("get_semantic_map");
      if (map.projects.length > 0) {
        setSemanticMap(map);
        setProjects(
          map.projects.map((row) => ({
            name: row.name,
            root_path: row.repo_path || null,
            nodes: row.node_count,
            edges: row.edge_count,
            files: row.files,
          })),
        );
        const fromMap = map.projects.flatMap((row) => row.dead);
        try {
          const listed = await invoke<DeadSymbol[]>("get_dead_symbols");
          setDeadSymbols(listed.length > 0 ? listed : fromMap);
        } catch {
          setDeadSymbols(fromMap);
        }
        return;
      }
      const rows = await invoke<ProjectSummary[]>("list_projects");
      if (rows.length > 0) {
        setProjects(rows);
      }
    } catch {
      try {
        const rows = await invoke<ProjectSummary[]>("list_projects");
        if (rows.length > 0) {
          setProjects(rows);
        }
      } catch {
        /* CBM/SQLite henüz boş olabilir */
      }
    }
    try {
      setDeadSymbols(await invoke<DeadSymbol[]>("get_dead_symbols"));
    } catch {
      setDeadSymbols([]);
    }
  }, []);

  const syncInstalledModels = useCallback(async () => {
    if (!isTauri()) {
      setModels([]);
      return;
    }
    try {
      const listed = await invoke<string[]>("list_ollama_models");
      const unique = Array.from(new Set(listed.filter((name) => name.trim().length > 0))).sort(
        (left, right) => left.localeCompare(right),
      );
      setModels(unique);
      const current = (await invoke<string>("get_kernel_model")).trim();
      if (unique.includes(current)) {
        setModel(current);
        return;
      }
      const fallback = unique[0];
      if (!fallback) {
        setModel("");
        return;
      }
      const saved = await invoke<string>("set_kernel_model", { model: fallback });
      setModel(saved);
    } catch {
      /* LMR geçici olarak yanıt vermezse önceki listeyi koru */
    }
  }, []);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      return;
    }
    setKernel("booting");
    try {
      const next = await invoke<ServiceReport>("ensure_services");
      setReport(next);
      setKernel(next.ollama.running && next.nats.running ? "ready" : "degraded");
      await syncInstalledModels();
      try {
        setDecisionGate(await invoke<DecisionGateStatus>("get_decision_gate_status"));
      } catch {
        /* DecisionGate henüz yönetilmiyor olabilir */
      }
      try {
        const engine = await invoke<LayaEngineStatus>("get_laya_engine_status");
        setLayaEngine(engine);
        if (engine.phase !== "ready") {
          void invoke<LayaEngineStatus>("ensure_laya_engine")
            .then((next) => setLayaEngine(next))
            .catch(() => {
              /* indirme zaten kernel tarafında yürüyor olabilir */
            });
        }
      } catch {
        /* Laya Engine henüz yönetilmiyor olabilir */
      }
      try {
        const state = await invoke<QuotaState>("get_quota_state");
        setQuotas(state.quotas);
        setAmberAlert(state.amber_alert);
        setAmberTools(state.amber_tools);
      } catch {
        try {
          const rows = await invoke<ToolQuota[]>("list_quotas");
          if (rows.length > 0) {
            setQuotas(rows);
            const amber = rows.filter((row) => (row.percent ?? 0) >= AMBER_THRESHOLD).map((row) => row.id);
            setAmberAlert(amber.length > 0);
            setAmberTools(amber);
          }
        } catch {
          setQuotas(MOCK_QUOTAS);
        }
      }
      try {
        setPolicy(await invoke<RoutingPolicy>("get_routing_policy"));
      } catch {
        setPolicy(DEFAULT_POLICY);
      }
      await refreshSemantic();
    } catch (error) {
      setKernel("error");
      console.error(error);
    }
  }, [refreshSemantic, syncInstalledModels]);

  const applyModel = useCallback(async (next?: string) => {
    const chosen = (next ?? model).trim();
    if (!chosen) {
      return;
    }
    if (!isTauri()) {
      setModel(chosen);
      return;
    }
    try {
      const saved = await invoke<string>("set_kernel_model", { model: chosen });
      setModel(saved);
    } catch (error) {
      console.error(error);
    }
  }, [model]);

  const enableLaya = useCallback(async () => {
    if (!isTauri()) {
      return;
    }
    setDecisionGate({
      phase: "loading",
      title: "OpenJev Laya yükleniyor",
      message: "Karar motoru ağırlıkları okunuyor. Bu sırada kernel LMR ile çalışır.",
      detail: null,
      device: null,
      reason: null,
    });
    try {
      setDecisionGate(await invoke<DecisionGateStatus>("enable_decision_gate"));
    } catch (error) {
      console.error(error);
      try {
        setDecisionGate(await invoke<DecisionGateStatus>("get_decision_gate_status"));
      } catch (statusError) {
        console.error(statusError);
      }
    }
  }, []);

  const declineLaya = useCallback(async () => {
    if (!isTauri()) {
      return;
    }
    try {
      setDecisionGate(await invoke<DecisionGateStatus>("decline_decision_gate"));
    } catch (error) {
      console.error(error);
    }
  }, []);

  const savePolicy = useCallback(async (next: RoutingPolicy) => {
    const locked = { ...next, require_user_approval: true };
    if (!isTauri()) {
      setPolicy(locked);
      return;
    }
    const saved = await invoke<RoutingPolicy>("set_routing_policy", { policy: locked });
    setPolicy(saved);
  }, []);

  const ingestBusMessage = useCallback((message: LoungeMessage) => {
    const row = loungeMessageToEvent(message);
    setEvents((current) => {
      if (current.some((event) => event.id === row.id)) {
        return current;
      }
      return [row, ...current].slice(0, EVENT_CAP);
    });
    setDecisionMsgTimes((times) => recordMsgTick(times, message.id));
    const telemetry = parseDecisionTelemetry(message);
    if (telemetry && lastTelemetryId.current !== message.id) {
      lastTelemetryId.current = message.id;
      setDecisionTelemetry(telemetry);
      setDecisionLatencyHistory((history) =>
        [...history, telemetry.latency_ms].slice(-LATENCY_SPARK_CAP),
      );
    }
    const engine = parseLayaEngineStatus(message);
    if (engine) {
      setLayaEngine(engine);
    }
    const security = parseSecurityAlert(message);
    if (security) {
      setApproval(security);
    }
    const whisper = parseContextWhisper(message);
    if (whisper) {
      setWhisperedExperienceIds(whisper.experienceIds);
      if (whisper.experiences.length > 0) {
        setExperiences((current) => mergeWhisperedExperiences(current, whisper));
      }
    }
    if (row.subject.includes("experience")) {
      void refreshSemantic();
    }
  }, [refreshSemantic]);

  const indexWorkspace = useCallback(async () => {
    if (indexing) {
      return;
    }
    const repoPath = isTauri()
      ? await pickWorkspaceFolder()
      : "/Users/demo/Agent-Lounge-OS";
    if (!repoPath) {
      return;
    }
    setIndexing(true);
    setIndexNotice(null);
    try {
      const snapshot = isTauri()
        ? await invoke<IndexSnapshot>("index_workspace", { path: repoPath })
        : await new Promise<IndexSnapshot>((resolve) => {
            window.setTimeout(() => resolve(mockIndexSnapshot(repoPath)), 1400);
          });
      setLastIndex(snapshot);
      setProjects((current) => upsertIndexedProject(current, snapshot, repoPath));
      if (isTauri()) {
        await refreshSemantic();
      } else {
        setDeadSymbols(mockDeadSymbols(snapshot));
      }
      const files = snapshot.files ?? 0;
      ingestBusMessage(
        indexBusMessage("lounge.index.completed", {
          path: repoPath,
          project: snapshot.project,
          files,
          dead: snapshot.dead ?? 0,
        }),
      );
      setIndexNotice({
        tone: "success",
        text: `Success · ${files} files · ${snapshot.project || repoPath}`,
      });
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      ingestBusMessage(
        indexBusMessage("lounge.index.failed", {
          path: repoPath,
          error: text,
        }),
      );
      setIndexNotice({ tone: "error", text });
    } finally {
      setIndexing(false);
    }
  }, [indexing, ingestBusMessage, refreshSemantic]);

  const probeBus = useCallback(async () => {
    if (!isTauri()) {
      ingestBusMessage({
        id: crypto.randomUUID(),
        type: "bus",
        subject: "lounge.bus.probe",
        source_agent: "browser",
        target_agent: "bus",
        created_at: new Date().toISOString(),
        payload: { kind: "probe" },
        payload_bytes: 16,
      });
      return;
    }
    await invoke<LoungeMessage>("probe_bus");
  }, [ingestBusMessage]);

  const resolveApproval = useCallback(async (vote: RoutingVote) => {
    if (!approval) {
      return;
    }
    if (isTauri()) {
      await invoke("resolve_routing", { taskId: approval.task_id, vote });
    }
    setApproval(null);
  }, [approval]);

  useEffect(() => {
    if (!indexNotice) {
      return;
    }
    const id = window.setTimeout(() => setIndexNotice(null), 4000);
    return () => window.clearTimeout(id);
  }, [indexNotice]);

  useEffect(() => {
    const boot = window.setTimeout(() => {
      setClock(formatClock(new Date()));
      if (isTauri()) {
        setEvents([]);
      } else {
        setKernel("browser");
      }
      void refresh();
    }, 0);
    const id = window.setInterval(() => setClock(formatClock(new Date())), 30_000);
    const modelsId = window.setInterval(() => {
      if (isTauri()) {
        void syncInstalledModels();
      }
    }, 20_000);
    const meterId = window.setInterval(() => {
      setDecisionMsgTimes((times) => {
        const next = pruneMsgWindow(times);
        return next.length === times.length ? times : next;
      });
    }, 5_000);
    return () => {
      window.clearTimeout(boot);
      window.clearInterval(id);
      window.clearInterval(modelsId);
      window.clearInterval(meterId);
    };
  }, [refresh, syncInstalledModels]);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];

    void (async () => {
      try {
        unlisteners.push(
          await listen<LoungeMessage>(BUS_UI_EVENT, (event) => {
            if (!cancelled) {
              ingestBusMessage(event.payload);
            }
          }),
        );
        unlisteners.push(
          await listen<QuotaState>(QUOTA_UI_EVENT, (event) => {
            if (!cancelled) {
              setQuotas(event.payload.quotas);
              setAmberAlert(event.payload.amber_alert);
              setAmberTools(event.payload.amber_tools);
            }
          }),
        );
        unlisteners.push(
          await listen<ServiceReport>(SERVICE_UI_EVENT, (event) => {
            if (!cancelled) {
              setReport(event.payload);
              setKernel(
                event.payload.ollama.running && event.payload.nats.running ? "ready" : "degraded",
              );
              if (event.payload.ollama.running) {
                void syncInstalledModels();
              } else {
                setModels([]);
              }
            }
          }),
        );
        unlisteners.push(
          await listen<PullProgress>(MODEL_PULL_EVENT, (event) => {
            if (!cancelled && event.payload.done && !event.payload.error) {
              void syncInstalledModels();
            }
          }),
        );
        unlisteners.push(
          await listen<DecisionGateStatus>(DECISION_GATE_EVENT, (event) => {
            if (!cancelled) {
              setDecisionGate(event.payload);
            }
          }),
        );
        unlisteners.push(
          await listen<LayaEngineStatus>(LAYA_ENGINE_EVENT, (event) => {
            if (!cancelled) {
              setLayaEngine(event.payload);
            }
          }),
        );
        unlisteners.push(
          await listen<ApprovalRequest>("lounge://routing-approval", (event) => {
            if (!cancelled) {
              setApproval(event.payload);
            }
          }),
        );
      } catch (error) {
        console.error(error);
      }
    })();

    return () => {
      cancelled = true;
      unlisteners.forEach((fn) => {
        void fn();
      });
    };
  }, [ingestBusMessage, syncInstalledModels]);

  const decisionMsgPerMin = decisionMsgTimes.length;

  const value = useMemo<LoungeContextValue>(
    () => ({
      report,
      kernel,
      model,
      setModel,
      models,
      experiences,
      whisperedExperienceIds,
      events,
      quotas,
      amberAlert,
      amberTools,
      projects,
      lastIndex,
      semanticMap,
      deadSymbols,
      query,
      setQuery,
      clock,
      indexing,
      indexNotice,
      policy,
      approval,
      decisionGate,
      layaEngine,
      decisionTelemetry,
      decisionLatencyHistory,
      decisionMsgPerMin,
      applyModel,
      enableLaya,
      declineLaya,
      indexWorkspace,
      refresh,
      savePolicy,
      resolveApproval,
      ingestBusMessage,
      probeBus,
    }),
    [
      report,
      kernel,
      model,
      models,
      experiences,
      whisperedExperienceIds,
      events,
      quotas,
      amberAlert,
      amberTools,
      projects,
      lastIndex,
      semanticMap,
      deadSymbols,
      query,
      clock,
      indexing,
      indexNotice,
      policy,
      approval,
      decisionGate,
      layaEngine,
      decisionTelemetry,
      decisionLatencyHistory,
      decisionMsgPerMin,
      applyModel,
      enableLaya,
      declineLaya,
      indexWorkspace,
      refresh,
      savePolicy,
      resolveApproval,
      ingestBusMessage,
      probeBus,
    ],
  );

  return <LoungeContext.Provider value={value}>{children}</LoungeContext.Provider>;
}

export function useLounge(): LoungeContextValue {
  const value = useContext(LoungeContext);
  if (!value) {
    throw new Error("useLounge must be used within LoungeProvider");
  }
  return value;
}

function upsertIndexedProject(
  current: ProjectSummary[],
  snapshot: IndexSnapshot,
  repoPath: string,
): ProjectSummary[] {
  const next: ProjectSummary = {
    name: snapshot.project || repoPath.split(/[/\\]/).filter(Boolean).at(-1) || "workspace",
    root_path: repoPath,
    nodes: snapshot.nodes,
    edges: snapshot.edges,
    files: snapshot.files ?? null,
  };
  const index = current.findIndex(
    (row) => row.name === next.name || row.root_path === repoPath,
  );
  if (index === -1) {
    return [next, ...current];
  }
  const copy = current.slice();
  copy[index] = { ...copy[index], ...next };
  return copy;
}

function mockDeadSymbols(snapshot: IndexSnapshot): DeadSymbol[] {
  const count = snapshot.dead ?? 0;
  return Array.from({ length: count }, (_, index) => ({
    name: `unused_symbol_${index + 1}`,
    kind: "unused",
    file: "src/lib.rs",
    line: index + 1,
    detail: null,
    project_id: snapshot.project,
  }));
}
