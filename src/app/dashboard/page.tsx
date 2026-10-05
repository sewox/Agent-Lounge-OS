"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CollapsiblePanel } from "@/components/collapsible-panel";
import { EventStreamPanel, OverviewKpis, QuotaMiniCard, VaultPanel } from "@/components/panels";
import { useIsTauri } from "@/hooks/use-is-tauri";

type DashSession = {
  id: string;
  agent_id: string;
  state: string;
  orchestrated: boolean;
};

type DashBgTask = {
  id: string;
  status: string;
  summary: string;
};

function OrchestrationMiniPanel() {
  const { t } = useTranslation("fleet");
  const tauriHost = useIsTauri();
  const [sessions, setSessions] = useState<DashSession[]>([]);
  const [tasks, setTasks] = useState<DashBgTask[]>([]);

  useEffect(() => {
    if (!tauriHost) return;
    let cancelled = false;
    const load = async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const [sessionRows, taskRows] = await Promise.all([
          invoke<DashSession[]>("list_agent_sessions").catch(() => [] as DashSession[]),
          invoke<DashBgTask[]>("list_a2a_background_tasks").catch(() => [] as DashBgTask[]),
        ]);
        if (!cancelled) {
          setSessions(sessionRows);
          setTasks(taskRows);
        }
      } catch {
        if (!cancelled) {
          setSessions([]);
          setTasks([]);
        }
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 8_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [tauriHost]);

  return (
    <div data-qa="dash-orchestration" className="space-y-2 font-mono text-meta">
      <ul data-qa="dash-sessions" className="max-h-28 space-y-1 overflow-auto">
        {sessions.length === 0 ? (
          <li className="text-outline">{t("noSessions")}</li>
        ) : (
          sessions.map((session) => (
            <li
              key={session.id}
              data-qa="dash-session-row"
              data-orchestrated={session.orchestrated ? "true" : "false"}
              className="flex flex-wrap items-center gap-1.5 text-on-surface-variant"
            >
              <span className="truncate text-on-surface">{session.agent_id || session.id}</span>
              <span>{session.state}</span>
              {session.orchestrated ? (
                <span
                  data-qa="orchestrated-label"
                  className="rounded border border-secondary/50 bg-secondary-container/30 px-1.5 py-0.5 font-body font-semibold tracking-label text-secondary uppercase"
                >
                  {t("orchestrated")}
                </span>
              ) : null}
            </li>
          ))
        )}
      </ul>
      <ul data-qa="dash-background-tasks" className="max-h-28 space-y-1 overflow-auto border-t border-outline-variant/40 pt-2">
        {tasks.length === 0 ? (
          <li className="text-outline">{t("noBackgroundTasks")}</li>
        ) : (
          tasks.map((task) => (
            <li
              key={task.id}
              data-qa="dash-background-task"
              data-task-status={task.status}
              className="flex flex-wrap items-center gap-1.5 text-on-surface-variant"
            >
              <span className="rounded border border-outline-variant px-1 py-0.5 font-body uppercase text-on-surface">
                {task.status}
              </span>
              <span className="truncate">{task.summary || task.id}</span>
            </li>
          ))
        )}
      </ul>
    </div>
  );
}

export default function DashboardPage() {
  const { t } = useTranslation("dashboard");
  const { t: tf } = useTranslation("fleet");
  return (
    <div className="flex min-h-[calc(100vh-4.5rem)] w-full min-h-0 flex-col gap-3 pb-4">
      <div className="w-full shrink-0">
        <OverviewKpis />
      </div>
      {/* ≥960px + landscape: side-by-side (DB-04 / D960). Portrait (D3) stays stacked for L6. */}
      <div className="grid w-full flex-1 grid-cols-1 gap-3 min-[960px]:landscape:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)] min-[960px]:landscape:items-stretch">
        <CollapsiblePanel
          id="dash-event-stream"
          title={t("panels.eventStream")}
          className="flex h-full min-h-[20rem] w-full flex-col"
          bodyClassName="flex min-h-[16rem] flex-1 flex-col min-[960px]:landscape:min-h-[var(--panel-min-h)]"
        >
          <EventStreamPanel embedded />
        </CollapsiblePanel>
        <div className="flex h-full w-full min-w-0 flex-col gap-3">
          <CollapsiblePanel
            id="dash-vault"
            title={t("panels.vault")}
            className="flex min-h-[var(--panel-min-h)] w-full flex-1 flex-col"
            bodyClassName="flex min-h-[var(--panel-min-h)] flex-1 flex-col"
          >
            <VaultPanel embedded />
          </CollapsiblePanel>
          <CollapsiblePanel
            id="dash-orchestration"
            title={tf("orchestrationTitle")}
            className="w-full shrink-0"
            trailer={
              <Link
                href="/fleet"
                className="font-body text-meta font-medium text-primary hover:underline"
              >
                Fleet
              </Link>
            }
            bodyClassName="flex flex-col"
          >
            <OrchestrationMiniPanel />
          </CollapsiblePanel>
          <CollapsiblePanel
            id="dash-quota"
            title={t("panels.quotas")}
            className="w-full shrink-0"
            trailer={
              <Link
                href="/quotas"
                className="font-body text-meta font-medium text-primary hover:underline"
              >
                {t("panels.allQuotas")}
              </Link>
            }
            bodyClassName="flex flex-col"
          >
            <QuotaMiniCard />
          </CollapsiblePanel>
        </div>
      </div>
    </div>
  );
}
