"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { useEffect, useRef, useState } from "react";
import { BrandMark } from "@/components/brand";
import { Icon } from "@/components/icons";
import { useLounge } from "@/components/lounge-provider";
import { daemonLabel, daemonTone, Pip } from "@/components/ui";
import {
  isSecurityApproval,
  isQuotaApproval,
  approvalRemainingSecs,
  SECURITY_OVERLAY_PROMPT,
  QUOTA_ALERT_PROMPT,
  QUOTA_CONTINUE_LOCAL_LABEL,
  coreServicesDegraded,
  degradedCoreServiceNames,
  type DecisionGateStatus,
} from "@/lib/lounge";

export type NavId =
  | "dashboard"
  | "stream"
  | "vault"
  | "health"
  | "fleet"
  | "telemetry"
  | "quotas"
  | "settings";

const NAV: { id: NavId; href: string; label: string; icon: string }[] = [
  { id: "dashboard", href: "/dashboard", label: "Dashboard", icon: "dashboard" },
  { id: "stream", href: "/stream", label: "Event Stream", icon: "stream" },
  { id: "vault", href: "/vault", label: "Knowledge Vault", icon: "db" },
  { id: "health", href: "/health", label: "Project Health", icon: "health" },
  { id: "fleet", href: "/fleet", label: "Worker Fleet", icon: "chip" },
  { id: "telemetry", href: "/telemetry", label: "Telemetry", icon: "chart" },
  { id: "quotas", href: "/quotas", label: "Quotas", icon: "pie" },
  { id: "settings", href: "/settings", label: "Settings", icon: "gear" },
];

const TITLES: Record<string, string> = {
  "/stream": "Event Stream",
  "/vault": "Knowledge Vault",
  "/health": "Project Health",
  "/fleet": "Worker Fleet",
  "/telemetry": "Telemetry",
  "/quotas": "Quotas",
  "/settings": "Settings",
  "/onboarding": "Onboarding",
  "/dashboard": "Dashboard",
};

export function AppShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const {
    report,
    kernel,
    model,
    models,
    query,
    setQuery,
    clock,
    indexing,
    indexNotice,
    quotas,
    amberAlert,
    amberTools,
    approval,
    approvalError,
    decisionGate,
    layaEngine,
    decisionTelemetry,
    applyModel,
    enableLaya,
    declineLaya,
    indexWorkspace,
    resolveApproval,
    setOpenCommandPalette,
  } = useLounge();

  const crumb = TITLES[pathname] ?? "Lounge";
  const warnQuota = quotas.find((row) => (row.percent ?? 0) >= 80);
  const onboarding = pathname === "/onboarding";
  const [layaDismissed, setLayaDismissed] = useState(false);
  const [approvalSecsLeft, setApprovalSecsLeft] = useState<number | null>(null);
  const layaPhase = useRef(decisionGate?.phase);
  useEffect(() => {
    if (layaPhase.current !== decisionGate?.phase) {
      layaPhase.current = decisionGate?.phase;
      setLayaDismissed(false);
    }
  }, [decisionGate?.phase]);
  useEffect(() => {
    if (!approval) {
      const clearId = window.setTimeout(() => setApprovalSecsLeft(null), 0);
      return () => window.clearTimeout(clearId);
    }
    const tick = () => setApprovalSecsLeft(approvalRemainingSecs(approval));
    const bootId = window.setTimeout(tick, 0);
    const intervalId = window.setInterval(tick, 1000);
    return () => {
      window.clearTimeout(bootId);
      window.clearInterval(intervalId);
    };
  }, [approval]);
  const countdownLabel =
    approvalSecsLeft != null ? ` · ${approvalSecsLeft}s` : "";
  const layaBanner =
    decisionGate?.phase === "available" ||
    (Boolean(decisionGate) &&
      decisionGate?.phase !== "ready" &&
      !layaDismissed &&
      layaEngine?.phase !== "downloading");
  const securityHold = Boolean(approval && isSecurityApproval(approval.kind));
  const quotaHold = Boolean(approval && isQuotaApproval(approval.kind));
  const serviceDegraded = coreServicesDegraded(report);
  const degradedNames = degradedCoreServiceNames(report);
  const statusBanner =
    serviceDegraded ||
    (approval && !securityHold && !quotaHold) ||
    indexing ||
    indexNotice ||
    layaBanner ||
    Boolean(approvalError);
  const bannerOffset = Boolean(statusBanner);
  const bannerPos = onboarding
    ? "fixed inset-x-0 top-12 z-[55] pointer-events-auto"
    : "fixed top-12 right-0 left-[var(--sidebar-w)] z-[55] pointer-events-auto";

  return (
    <div className="flex h-screen overflow-hidden bg-surface text-on-surface">
      {securityHold && approval ? (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center bg-surface-container-lowest/80 px-4 backdrop-blur-sm pointer-events-auto"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="security-overlay-title"
          data-approval-chrome="security"
        >
          <div className="relative z-[61] w-full max-w-md border border-error-container bg-surface-container-high p-5 shadow-lg pointer-events-auto">
            <p className="font-mono text-[10px] font-bold tracking-[0.2em] text-error-dim uppercase">
              Security · {approval.kind === "security_risky" ? "Risky" : "Critical"} ·
              PENDING_APPROVAL{countdownLabel}
            </p>
            <h2
              id="security-overlay-title"
              className="mt-3 font-body text-base font-semibold text-on-surface"
            >
              {SECURITY_OVERLAY_PROMPT}
            </h2>
            <p className="mt-2 truncate font-mono text-[11px] text-outline">
              {approval.from_agent} · {approval.summary}
            </p>
            <div className="relative z-[62] mt-5 flex items-center justify-end gap-2 pointer-events-auto">
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("deny", approval.task_id)}
                className="rounded border border-error bg-error-container px-3 py-1.5 font-mono text-[11px] text-on-error-container"
              >
                Reddet
              </button>
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("approve", approval.task_id)}
                className="rounded bg-primary-container px-3 py-1.5 font-mono text-[11px] font-semibold text-on-primary-container"
              >
                Onayla
              </button>
            </div>
          </div>
        </div>
      ) : null}

      {quotaHold && approval ? (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center bg-surface-container-lowest/80 px-4 backdrop-blur-sm pointer-events-auto"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="quota-overlay-title"
          data-approval-chrome="quota"
        >
          <div className="relative z-[61] w-full max-w-md border border-outline-variant bg-surface-container-high p-5 shadow-lg pointer-events-auto">
            <p className="font-mono text-[10px] font-bold tracking-[0.2em] text-tertiary uppercase">
              Quota Alert · QUOTA_BLOCKED{countdownLabel}
            </p>
            <h2
              id="quota-overlay-title"
              className="mt-3 font-body text-base font-semibold text-on-surface"
            >
              {QUOTA_ALERT_PROMPT}
            </h2>
            <p className="mt-2 font-mono text-[11px] text-outline">
              {approval.from_agent} → {approval.to_agent} · {approval.summary}
            </p>
            <p className="mt-1 font-mono text-[10px] text-outline/80">{approval.reason}</p>
            <div className="relative z-[62] mt-5 flex flex-wrap items-center justify-end gap-2 pointer-events-auto">
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("deny", approval.task_id)}
                className="rounded border border-outline-variant bg-surface-container px-3 py-1.5 font-mono text-[11px] text-on-surface"
              >
                Kapat
              </button>
              {approval.kind === "quota_local_fallback" ? (
                <button
                  type="button"
                  data-task-id={approval.task_id}
                  onClick={() => void resolveApproval("approve_local", approval.task_id)}
                  className="rounded bg-primary-container px-3 py-1.5 font-mono text-[11px] font-semibold text-on-primary-container"
                >
                  {QUOTA_CONTINUE_LOCAL_LABEL}
                </button>
              ) : null}
            </div>
          </div>
        </div>
      ) : null}

      {serviceDegraded ? (
        <div
          className={`${bannerPos} border-b border-error-container bg-error-container/25 py-2 px-4`}
          role="status"
          aria-live="polite"
        >
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 px-4 font-mono text-[11px]">
            <div className="min-w-0 truncate text-on-surface">
              <span className="font-bold tracking-wider text-error-dim uppercase">
                Service Degraded ·{" "}
              </span>
              <span className="text-on-surface">
                {degradedNames.join(", ")} kapalı — otomatik yeniden başlatma deneniyor
              </span>
              {degradedNames.includes("LMR") && report?.ollama.error ? (
                <span className="ml-2 truncate text-outline" title={report.ollama.error}>
                  · {report.ollama.error}
                </span>
              ) : null}
              {degradedNames.includes("NATS") && report?.nats.error ? (
                <span className="ml-2 truncate text-outline" title={report.nats.error}>
                  · {report.nats.error}
                </span>
              ) : null}
            </div>
            <span className="shrink-0 rounded border border-error-container/60 bg-surface-container-high px-2 py-0.5 text-[10px] tracking-wider text-error-dim uppercase">
              auto-restart
            </span>
          </div>
        </div>
      ) : approval && !securityHold && !quotaHold ? (
        <div
          className={`${bannerPos} border-b border-error-container bg-error-container/20 py-2 px-4`}
          data-approval-chrome="routing"
        >
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 px-4 font-mono text-[11px]">
            <div className="min-w-0 truncate text-on-surface">
              <span className="font-bold uppercase text-error-dim">
                Routing onayı{countdownLabel} ·{" "}
              </span>
              {approval.from_agent} → {approval.to_agent} · {approval.summary}
              <span className="ml-2 text-outline">{approval.reason}</span>
            </div>
            <div className="relative z-[56] flex shrink-0 items-center gap-1.5 pointer-events-auto">
              {approval.kind === "agent_switch" ? (
                <button
                  type="button"
                  data-task-id={approval.task_id}
                  onClick={() => void resolveApproval("approve_local", approval.task_id)}
                  className="rounded border border-outline-variant bg-surface-container-high px-2 py-1 text-on-surface hover:bg-surface-bright"
                >
                  Yerel modele geç
                </button>
              ) : null}
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("approve", approval.task_id)}
                className="rounded bg-primary-container px-2 py-1 font-semibold text-on-primary-container"
              >
                Onayla
              </button>
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("deny", approval.task_id)}
                className="rounded border border-error bg-error-container px-2 py-1 text-on-error-container"
              >
                Reddet
              </button>
            </div>
          </div>
        </div>
      ) : indexing ? (
        <div className={`${bannerPos} border-b border-outline-variant bg-surface-container-low py-2 px-4`}>
          <div className="flex min-w-0 items-center gap-2 px-4 font-mono text-[11px] text-on-surface">
            <span
              className="h-3.5 w-3.5 shrink-0 animate-spin rounded-full border-2 border-transparent border-t-primary"
              aria-hidden
            />
            <span className="shrink-0 font-bold tracking-wider uppercase">Scanning...</span>
            <span className="truncate text-outline">memory_bridge · index_workspace</span>
          </div>
        </div>
      ) : indexNotice ? (
        <div
          className={`${bannerPos} border-b py-2 px-4 ${
            indexNotice.tone === "error"
              ? "border-error-container bg-error-container/20"
              : "border-secondary-container bg-secondary-container/30"
          }`}
        >
          <div
            className={`truncate px-4 font-mono text-[11px] font-semibold ${
              indexNotice.tone === "error" ? "text-error-dim" : "text-secondary-dim"
            }`}
          >
            {indexNotice.text}
          </div>
        </div>
      ) : approvalError ? (
        <div
          className={`${bannerPos} border-b border-error-container bg-error-container/20 py-2 px-4`}
          role="status"
        >
          <div className="truncate px-4 font-mono text-[11px] font-semibold text-error-dim">
            {approvalError}
          </div>
        </div>
      ) : layaBanner && decisionGate ? (
        <div
          className={`${bannerPos} border-b py-2 px-4 ${
            decisionGate.phase === "failed"
              ? "border-error-container bg-error-container"
              : decisionGate.phase === "available"
                ? "border-secondary-container bg-secondary-container"
                : "border-outline-variant bg-surface-container-low"
          }`}
        >
          <div className="flex min-w-0 items-center justify-between gap-3 px-4 font-mono text-[11px]">
            <div
              className="min-w-0 truncate"
              title={decisionGate.detail ? `${decisionGate.message} · ${decisionGate.detail}` : decisionGate.message}
            >
              <span
                className={`font-bold tracking-wider uppercase ${
                  decisionGate.phase === "failed" ? "text-on-error-container" : "text-on-surface"
                }`}
              >
                {decisionGate.title} ·{" "}
              </span>
              <span className={decisionGate.phase === "failed" ? "text-on-error-container" : "text-on-surface"}>
                {decisionGate.message}
              </span>
            </div>
            {decisionGate.phase === "available" ? (
              <div className="flex shrink-0 items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => void enableLaya()}
                  className="rounded bg-primary-container px-2 py-1 font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed"
                >
                  Laya&apos;ya geç
                </button>
                <button
                  type="button"
                  onClick={() => void declineLaya()}
                  className="rounded border border-outline-variant bg-surface-container-high px-2 py-1 text-on-surface hover:bg-surface-bright"
                >
                  Şimdilik LMR
                </button>
              </div>
            ) : decisionGate.phase === "failed" ? (
              <div className="flex shrink-0 items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => void enableLaya()}
                  className="rounded bg-primary-container px-2 py-1 font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed"
                >
                  Yeniden dene
                </button>
                <button
                  type="button"
                  onClick={() => setLayaDismissed(true)}
                  className="shrink-0 rounded border border-outline-variant bg-surface-container-high px-2 py-1 text-on-surface hover:bg-surface-bright"
                >
                  Gizle
                </button>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => setLayaDismissed(true)}
                className="shrink-0 rounded border border-outline-variant bg-surface-container-high px-2 py-1 text-on-surface hover:bg-surface-bright"
              >
                Gizle
              </button>
            )}
          </div>
        </div>
      ) : null}

      <header className="fixed top-0 left-0 z-40 flex h-12 w-full min-w-0 items-center gap-2 border-b border-outline-variant bg-surface px-3">
        <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden">
          <div className="flex min-w-0 items-center gap-2 border-r border-outline-variant pr-2">
            <BrandMark size={22} />
            <span className="truncate font-headline font-mono text-xs font-bold tracking-tight uppercase">
              Agent Lounge OS
            </span>
            <span className="hidden shrink-0 rounded border border-outline-variant bg-surface-container-high px-1 py-0.5 font-mono text-[9px] text-primary uppercase 2xl:inline">
              KERNEL
            </span>
          </div>
          <div className="hidden min-w-0 items-center gap-1.5 overflow-hidden font-mono text-xs text-on-surface-variant 2xl:flex">
            {onboarding ? (
              <span>Overview</span>
            ) : (
              <Link href="/dashboard" className="shrink-0 hover:text-on-surface">
                Overview
              </Link>
            )}
            <span className="shrink-0 text-outline">/</span>
            <span className="truncate font-medium text-on-surface">{crumb}</span>
          </div>
          {onboarding ? (
            <p className="min-w-0 flex-1 truncate font-mono text-[11px] text-on-surface-variant">
              Yerel araçları tarayıp Lounge’a bağlayın
            </p>
          ) : (
            <label className="relative hidden min-w-0 max-w-sm flex-1 xl:block">
              <span className="pointer-events-none absolute inset-y-0 left-2.5 flex items-center text-outline">
                <Icon name="search" />
              </span>
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
                    event.preventDefault();
                    setOpenCommandPalette(true);
                  }
                }}
                className="w-full min-w-0 rounded-lg border border-outline-variant bg-surface-container-low py-1 pr-12 pl-8 font-body text-xs text-on-surface placeholder:text-outline focus:border-primary focus:outline-none"
                placeholder="Filter subjects, repos, experiences"
              />
              <button
                type="button"
                onClick={() => setOpenCommandPalette(true)}
                className="absolute inset-y-0 right-1.5 flex items-center"
                title="Command Palette"
                aria-label="Open command palette"
              >
                <kbd className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-[10px] text-on-surface-variant hover:border-primary hover:text-primary">
                  ⌘K
                </kbd>
              </button>
            </label>
          )}
        </div>

        <div className="flex min-w-0 shrink-0 items-center gap-1.5">
          {onboarding ? null : (
            <>
              <ModelSelect
                model={model}
                models={models}
                lmrUp={report?.ollama.running === true}
                kernel={kernel}
                onChange={(next) => void applyModel(next)}
              />
              <div
                className={`hidden items-center gap-1 rounded border px-2 py-0.5 font-mono text-[11px] 2xl:flex ${
                  serviceDegraded
                    ? "border-error-container bg-error-container/20 text-error-dim"
                    : report?.nats.running
                      ? "border-outline-variant bg-surface-container-high text-primary"
                      : "border-outline-variant bg-surface-container-high text-on-surface-variant"
                }`}
                title={
                  serviceDegraded
                    ? `Service Degraded · ${degradedNames.join(", ")}`
                    : report?.nats.running
                      ? "NATS bağlı"
                      : "NATS kapalı"
                }
              >
                <span
                  className={`h-1.5 w-1.5 rounded-full ${
                    serviceDegraded ? "animate-pulse bg-error" : report?.nats.running ? "bg-secondary" : "bg-error"
                  }`}
                />
                <span>{serviceDegraded ? "DEGRADED" : "NATS"}</span>
              </div>
              <Link
                href="/quotas"
                title={
                  amberAlert
                    ? `AMBER ${Math.round(warnQuota?.percent ?? 80)}% ${amberTools[0] ?? warnQuota?.id ?? ""}`.trim()
                    : "Kota normal"
                }
                className={`flex min-w-0 max-w-[7.5rem] items-center gap-1 rounded border px-2 py-0.5 font-mono text-[11px] xl:max-w-[14rem] ${
                  amberAlert
                    ? "border-error-container bg-error-container/20 text-error-dim"
                    : "border-outline-variant bg-surface-container-high text-on-surface-variant"
                }`}
              >
                <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${amberAlert ? "animate-pulse bg-error" : "bg-secondary"}`} />
                <span className="truncate">
                  {amberAlert ? `AMBER ${Math.round(warnQuota?.percent ?? 80)}%` : "QUOTA OK"}
                </span>
              </Link>
              <div className="tnum hidden shrink-0 text-[11px] text-on-surface-variant 2xl:block">{clock} UTC+3</div>
              <button
                type="button"
                className="hidden items-center gap-1 rounded-lg border border-outline-variant bg-surface-container-high px-2.5 py-1 text-xs font-medium text-on-surface hover:bg-surface-bright 2xl:flex"
              >
                <Icon name="tune" />
                Quick Filter
              </button>
              <button
                type="button"
                onClick={() => void indexWorkspace()}
                disabled={indexing}
                className="flex shrink-0 items-center gap-1 rounded-lg bg-primary-container px-2.5 py-1 text-xs font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-60"
              >
                <Icon name="terminal" />
                <span className="whitespace-nowrap">{indexing ? "Scanning..." : "Index Workspace"}</span>
              </button>
            </>
          )}
          <span className="shrink-0 rounded-lg p-1 text-on-surface-variant" title={`kernel · ${kernel}`}>
            <Icon name="bell" className="h-[15px] w-[15px]" />
          </span>
        </div>
      </header>

      {onboarding ? null : (
      <aside className="fixed top-12 bottom-0 left-0 z-30 flex w-[var(--sidebar-w)] flex-col justify-between overflow-y-auto border-r border-outline-variant bg-surface-container-low px-2 py-3">
        <div className="space-y-4">
          <div className="flex items-center justify-between border-b border-outline-variant/60 px-2 pb-2">
            <div className="flex min-w-0 items-center gap-2">
              <BrandMark size={20} />
              <div>
                <div className="whitespace-nowrap font-headline text-xs font-black tracking-wider text-on-surface uppercase">
                  AL-OS CORE
                </div>
                <div className="font-mono text-[10px] text-on-surface-variant">
                  v0.1.0 · {serviceDegraded ? "degraded" : kernel}
                </div>
              </div>
            </div>
            <button
              type="button"
              className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-[10px] font-medium text-primary hover:bg-surface-bright"
            >
              + New Node
            </button>
          </div>
          <nav className="space-y-0.5 font-label text-xs">
            {NAV.map((item) => {
              const active =
                pathname === item.href || (item.id === "dashboard" && pathname === "/");
              return (
                <Link
                  key={item.id}
                  href={item.href}
                  className={`flex w-full items-center gap-2.5 rounded-lg px-3 py-1.5 text-left text-xs ${
                    active
                      ? "border-l-2 border-primary bg-surface-container-highest text-primary"
                      : "text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface"
                  }`}
                >
                  <Icon name={item.icon} />
                  <span className={`flex-1 font-body ${active ? "font-medium text-on-surface" : ""}`}>
                    {item.label}
                  </span>
                  {item.id === "stream" ? <span className="h-1.5 w-1.5 animate-ping rounded-full bg-primary" /> : null}
                </Link>
              );
            })}
          </nav>
        </div>

        <div className="space-y-3 border-t border-outline-variant/60 pt-3 pb-10">
          {serviceDegraded ? (
            <div
              className="rounded border border-error-container/70 bg-error-container/15 px-2 py-1.5 font-mono text-[10px] text-error-dim"
              role="status"
            >
              <div className="font-bold tracking-wider uppercase">Service Degraded</div>
              <div className="mt-0.5 truncate text-on-surface-variant">
                {degradedNames.join(" · ")} down
              </div>
            </div>
          ) : null}
          <div className="space-y-1.5 rounded border border-outline-variant/40 bg-surface-container-lowest/60 p-2">
            <div className="mb-1 font-mono text-[10px] tracking-wider text-outline uppercase">Active daemons</div>
            {[
              { name: "LMR", health: report?.ollama, fallback: "—" },
              { name: "NATS", health: report?.nats, fallback: "—" },
              { name: "Memory Bridge", health: report?.memory, fallback: "—" },
            ].map((daemon) => (
              <div key={daemon.name} className="flex items-center justify-between font-mono text-[11px]">
                <div className="flex items-center gap-2">
                  <Pip tone={daemon.name === "Memory Bridge" ? "primary" : daemonTone(daemon.health)} />
                  <span className="text-on-surface">{daemon.name}</span>
                </div>
                <span className={`tnum ${daemon.health?.running ? "text-secondary" : "text-error"}`}>
                  {daemonLabel(daemon.health, daemon.fallback)}
                </span>
              </div>
            ))}
            <div
              className="flex items-center justify-between font-mono text-[11px]"
              title={decisionGate?.message ?? "OpenJev Laya"}
            >
              <div className="flex min-w-0 items-center gap-2">
                <Pip
                  live={decisionGate?.phase === "loading" || decisionGate?.phase === "available"}
                  tone={
                    decisionGate?.phase === "ready"
                      ? "ok"
                      : decisionGate?.phase === "failed"
                        ? "down"
                        : "warn"
                  }
                />
                <span className="text-on-surface">OpenJev Laya</span>
              </div>
              <span
                className={`tnum ${
                  decisionGate?.phase === "ready"
                    ? "text-secondary"
                    : decisionGate?.phase === "failed"
                      ? "text-error"
                      : "text-on-surface-variant"
                }`}
              >
                {layaDaemonLabel(decisionGate, decisionTelemetry?.latency_ms)}
              </span>
            </div>
          </div>
          <div className="flex items-center justify-between border-t border-outline-variant/40 px-2 pt-1 font-body text-xs text-on-surface-variant">
            <span className="flex items-center gap-1">
              <Icon name="book" />
              Docs
            </span>
            <span className="flex items-center gap-1">
              <Icon name="key" />
              API Keys
            </span>
          </div>
        </div>
      </aside>
      )}

      <main className={`mt-12 flex h-[calc(100vh-3rem)] min-h-0 min-w-0 flex-col gap-3 overflow-hidden bg-surface p-3.5 ${onboarding ? "ml-0" : "ml-[var(--sidebar-w)]"} ${bannerOffset ? "pt-14" : ""}`}>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">{children}</div>
      </main>
    </div>
  );
}

function modelPickerLabel(kernel: string, lmrUp: boolean, count: number): string {
  if (kernel === "idle" || kernel === "booting") {
    return "Modeller…";
  }
  if (!lmrUp) {
    return "LMR kapalı";
  }
  if (count === 0) {
    return "Kurulu model yok";
  }
  return "Model seç";
}

function shortModelName(name: string): string {
  const trimmed = name.replace(/^hf\.co\//, "");
  const slash = trimmed.lastIndexOf("/");
  return slash >= 0 ? trimmed.slice(slash + 1) : trimmed;
}

function ModelSelect({
  model,
  models,
  lmrUp,
  kernel,
  onChange,
}: {
  model: string;
  models: string[];
  lmrUp: boolean;
  kernel: string;
  onChange: (next: string) => void;
}) {
  const usable = lmrUp && models.length > 0;
  const selected = models.includes(model) ? model : "";
  const label = modelPickerLabel(kernel, lmrUp, models.length);

  return (
    <label
      className="flex min-w-0 max-w-[11rem] items-center gap-1.5 rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 font-mono text-[11px] text-on-surface-variant xl:max-w-[18rem]"
      title={selected || label}
    >
      <span
        className={`h-1.5 w-1.5 shrink-0 rounded-full ${usable ? "animate-pulse bg-primary" : "bg-outline"}`}
      />
      <select
        aria-label="Kullanılan LMR modeli"
        value={selected}
        disabled={!usable}
        onChange={(event) => onChange(event.target.value)}
        className="min-w-0 flex-1 cursor-pointer bg-transparent text-on-surface outline-none disabled:cursor-not-allowed disabled:opacity-70"
      >
        <option value="" disabled>
          {label}
        </option>
        {models.map((item) => (
          <option key={item} value={item}>
            {shortModelName(item)}
          </option>
        ))}
      </select>
    </label>
  );
}

function layaDaemonLabel(
  status: DecisionGateStatus | null,
  latencyMs?: number | null,
): string {
  if (!status) {
    return "—";
  }
  if (status.phase === "ready") {
    if (latencyMs != null && Number.isFinite(latencyMs)) {
      return `${latencyMs.toFixed(1)}ms`;
    }
    return status.device || "ready";
  }
  if (status.phase === "loading") {
    return "yükleniyor";
  }
  if (status.phase === "available") {
    return "önerildi";
  }
  return "kapalı";
}
