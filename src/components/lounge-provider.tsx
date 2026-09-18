"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  DEFAULT_POLICY,
  formatClock,
  isTauri,
  MOCK_EVENTS,
  MOCK_EXPERIENCES,
  MOCK_QUOTAS,
  type ApprovalRequest,
  type IndexSnapshot,
  type LoungeExperience,
  type NatsEvent,
  type ProjectSummary,
  type RoutingPolicy,
  type RoutingVote,
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
  events: NatsEvent[];
  quotas: ToolQuota[];
  projects: ProjectSummary[];
  query: string;
  setQuery: (value: string) => void;
  clock: string;
  indexing: boolean;
  policy: RoutingPolicy;
  approval: ApprovalRequest | null;
  applyModel: () => Promise<void>;
  indexWorkspace: () => Promise<void>;
  refresh: () => Promise<void>;
  savePolicy: (next: RoutingPolicy) => Promise<void>;
  resolveApproval: (vote: RoutingVote) => Promise<void>;
};

const LoungeContext = createContext<LoungeContextValue | null>(null);

function asEventState(value: string): NatsEvent["state"] {
  if (value === "ok" || value === "error" || value === "retry" || value === "queued") {
    return value;
  }
  return "ok";
}

export function LoungeProvider({ children }: { children: ReactNode }) {
  const [report, setReport] = useState<ServiceReport | null>(null);
  const [kernel, setKernel] = useState(() => (isTauri() ? "idle" : "browser"));
  const [model, setModel] = useState("llama3.1:8b");
  const [models, setModels] = useState<string[]>([]);
  const [experiences, setExperiences] = useState<LoungeExperience[]>(MOCK_EXPERIENCES);
  const [events, setEvents] = useState<NatsEvent[]>(MOCK_EVENTS);
  const [quotas, setQuotas] = useState<ToolQuota[]>(MOCK_QUOTAS);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [query, setQuery] = useState("");
  const [clock, setClock] = useState(() => formatClock(new Date()));
  const [indexing, setIndexing] = useState(false);
  const [policy, setPolicy] = useState<RoutingPolicy>(DEFAULT_POLICY);
  const [approval, setApproval] = useState<ApprovalRequest | null>(null);

  const refreshSemantic = useCallback(async () => {
    if (!isTauri()) {
      return;
    }
    try {
      const rows = await invoke<LoungeExperience[]>("list_experiences", { limit: 12 });
      if (rows.length > 0) {
        setExperiences(rows);
      }
    } catch {
      /* SQLite henüz boş olabilir */
    }
    try {
      setProjects(await invoke<ProjectSummary[]>("list_projects"));
    } catch {
      setProjects([]);
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
      const current = await invoke<string>("get_kernel_model");
      setModel(current);
      try {
        setModels(await invoke<string[]>("list_ollama_models"));
      } catch {
        setModels([]);
      }
      try {
        const rows = await invoke<ToolQuota[]>("list_quotas");
        if (rows.length > 0) {
          setQuotas(rows);
        }
      } catch {
        setQuotas(MOCK_QUOTAS);
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
  }, [refreshSemantic]);

  const applyModel = useCallback(async () => {
    if (!isTauri()) {
      return;
    }
    const saved = await invoke<string>("set_kernel_model", { model });
    setModel(saved);
  }, [model]);

  const indexWorkspace = useCallback(async () => {
    if (!isTauri() || indexing) {
      return;
    }
    setIndexing(true);
    try {
      await invoke<IndexSnapshot>("index_workspace");
      await refreshSemantic();
      await refresh();
    } finally {
      setIndexing(false);
    }
  }, [indexing, refresh, refreshSemantic]);

  const savePolicy = useCallback(async (next: RoutingPolicy) => {
    const locked = { ...next, require_user_approval: true };
    if (!isTauri()) {
      setPolicy(locked);
      return;
    }
    const saved = await invoke<RoutingPolicy>("set_routing_policy", { policy: locked });
    setPolicy(saved);
  }, []);

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
    const boot = window.setTimeout(() => {
      void refresh();
    }, 0);
    const id = window.setInterval(() => setClock(formatClock(new Date())), 30_000);
    const quotaId = window.setInterval(() => {
      if (!isTauri()) {
        return;
      }
      void invoke<ToolQuota[]>("list_quotas")
        .then((rows) => {
          if (rows.length > 0) {
            setQuotas(rows);
          }
        })
        .catch(() => undefined);
    }, 15_000);
    return () => {
      window.clearTimeout(boot);
      window.clearInterval(id);
      window.clearInterval(quotaId);
    };
  }, [refresh]);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];

    void (async () => {
      try {
        unlisteners.push(
          await listen<NatsEvent>("lounge://nats", (event) => {
            if (cancelled) {
              return;
            }
            const payload = event.payload;
            const row: NatsEvent = {
              ...payload,
              state: asEventState(payload.state),
            };
            setEvents((current) => [row, ...current].slice(0, EVENT_CAP));
            if (row.subject.includes("experience")) {
              void refreshSemantic();
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
  }, [refreshSemantic]);

  const value = useMemo<LoungeContextValue>(
    () => ({
      report,
      kernel,
      model,
      setModel,
      models,
      experiences,
      events,
      quotas,
      projects,
      query,
      setQuery,
      clock,
      indexing,
      policy,
      approval,
      applyModel,
      indexWorkspace,
      refresh,
      savePolicy,
      resolveApproval,
    }),
    [
      report,
      kernel,
      model,
      models,
      experiences,
      events,
      quotas,
      projects,
      query,
      clock,
      indexing,
      policy,
      approval,
      applyModel,
      indexWorkspace,
      refresh,
      savePolicy,
      resolveApproval,
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
