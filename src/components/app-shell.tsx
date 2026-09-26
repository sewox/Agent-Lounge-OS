"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  isTauri,
  type ApprovalRequest,
  type DecisionGateStatus,
  type ServiceHealth,
  type ToolQuota,
} from "@/lib/lounge";
import { paletteShortcutLabel } from "@/lib/platform";
import { invoke } from "@tauri-apps/api/core";

const BANNER_BTN =
  "min-h-8 rounded px-2.5 py-1.5 font-body text-body pointer-events-auto";

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
  const [alertOpen, setAlertOpen] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [daemonsChecked, setDaemonsChecked] = useState(false);
  const shortcutLabel = paletteShortcutLabel();
  const alertRef = useRef<HTMLDivElement | null>(null);
  const layaPhase = useRef(decisionGate?.phase);

  useEffect(() => {
    const id = window.setTimeout(() => setDaemonsChecked(true), 400);
    return () => window.clearTimeout(id);
  }, []);

  useEffect(() => {
    if (!alertOpen) return;
    const onDoc = (event: MouseEvent) => {
      if (alertRef.current && !alertRef.current.contains(event.target as Node)) {
        setAlertOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setAlertOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [alertOpen]);

  const restartServices = useCallback(async () => {
    if (restarting) return;
    setRestarting(true);
    try {
      if (isTauri()) {
        await invoke("ensure_services");
      }
    } catch {
      // Supervisor will keep retrying; UI already shows Disconnected.
    } finally {
      setRestarting(false);
    }
  }, [restarting]);

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
  const criticalAlerts = useMemo(
    () => buildCriticalAlerts({ quotas, amberAlert, amberTools, approval, degradedNames }),
    [quotas, amberAlert, amberTools, approval, degradedNames],
  );
  const statusBanner =
    serviceDegraded ||
    (approval && !securityHold && !quotaHold) ||
    indexing ||
    indexNotice ||
    layaBanner ||
    Boolean(approvalError);
  const bannerRef = useRef<HTMLDivElement | null>(null);
  const [bannerHeight, setBannerHeight] = useState(0);
  const attachBannerRef = useCallback((node: HTMLDivElement | null) => {
    bannerRef.current = node;
  }, []);

  const degradedKey = degradedNames.join(",");
  useEffect(() => {
    if (!statusBanner) {
      const clearId = window.setTimeout(() => setBannerHeight(0), 0);
      return () => window.clearTimeout(clearId);
    }
    const el = bannerRef.current;
    if (!el) {
      return;
    }
    const measure = () => setBannerHeight(el.getBoundingClientRect().height);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [
    statusBanner,
    serviceDegraded,
    approval,
    indexing,
    indexNotice,
    approvalError,
    layaBanner,
    decisionGate,
    countdownLabel,
    degradedKey,
  ]);

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
            <p className="font-body text-meta font-bold tracking-label text-error-dim uppercase">
              Security · {approval.kind === "security_risky" ? "Risky" : "Critical"} ·
              PENDING_APPROVAL{countdownLabel}
            </p>
            <h2
              id="security-overlay-title"
              className="mt-3 font-body text-base font-semibold text-on-surface"
            >
              {SECURITY_OVERLAY_PROMPT}
            </h2>
            <p className="mt-2 whitespace-normal break-words font-body text-body leading-normal text-outline">
              <span className="font-mono">{approval.from_agent}</span> · {approval.summary}
            </p>
            <div className="relative z-[62] mt-5 flex flex-wrap items-center justify-end gap-2 pointer-events-auto">
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("deny", approval.task_id)}
                className={`${BANNER_BTN} border border-error bg-error-container text-on-error-container`}
              >
                Reddet
              </button>
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("approve", approval.task_id)}
                className={`${BANNER_BTN} bg-primary-container font-semibold text-on-primary-container`}
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
            <p className="font-body text-meta font-bold tracking-label text-tertiary uppercase">
              Quota Alert · QUOTA_BLOCKED{countdownLabel}
            </p>
            <h2
              id="quota-overlay-title"
              className="mt-3 font-body text-base font-semibold text-on-surface"
            >
              {QUOTA_ALERT_PROMPT}
            </h2>
            <p className="mt-2 whitespace-normal break-words font-body text-body leading-normal text-outline">
              <span className="font-mono">
                {approval.from_agent} → {approval.to_agent}
              </span>{" "}
              · {approval.summary}
            </p>
            <p className="mt-1 whitespace-normal break-words font-body text-meta leading-normal text-outline">
              {approval.reason}
            </p>
            <div className="relative z-[62] mt-5 flex flex-wrap items-center justify-end gap-2 pointer-events-auto">
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("deny", approval.task_id)}
                className={`${BANNER_BTN} border border-outline-variant bg-surface-container text-on-surface`}
              >
                Kapat
              </button>
              {approval.kind === "quota_local_fallback" ? (
                <button
                  type="button"
                  data-task-id={approval.task_id}
                  onClick={() => void resolveApproval("approve_local", approval.task_id)}
                  className={`${BANNER_BTN} bg-primary-container font-semibold text-on-primary-container`}
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
          ref={attachBannerRef}
          className={`${bannerPos} border-b border-error-container bg-error-container/25 py-2 px-4`}
          role="status"
          aria-live="polite"
        >
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-2 px-4 font-body text-body">
            <div className="min-w-0 flex-1 whitespace-normal break-words text-on-surface leading-normal">
              <span className="font-semibold tracking-label text-error-dim uppercase">
                Service Degraded ·{" "}
              </span>
              <span className="text-on-surface">
                {degradedNames.join(", ")} kapalı — otomatik yeniden başlatma deneniyor
              </span>
              {degradedNames.includes("LMR") && report?.ollama.error ? (
                <span className="ml-2 break-words text-outline">· {report.ollama.error}</span>
              ) : null}
              {degradedNames.includes("NATS") && report?.nats.error ? (
                <span className="ml-2 break-words text-outline">· {report.nats.error}</span>
              ) : null}
            </div>
            <span className="shrink-0 rounded border border-error-container/60 bg-surface-container-high px-2 py-0.5 text-meta tracking-label text-error-dim uppercase">
              auto-restart
            </span>
          </div>
        </div>
      ) : approval && !securityHold && !quotaHold ? (
        <div
          ref={attachBannerRef}
          className={`${bannerPos} border-b border-error-container bg-error-container/20 py-2 px-4`}
          data-approval-chrome="routing"
        >
          <div className="flex min-w-0 flex-wrap items-start justify-between gap-2 px-4 font-body text-body">
            <div className="min-w-0 flex-1 whitespace-normal break-words text-on-surface leading-normal">
              <span className="font-semibold tracking-label text-error-dim uppercase">
                Routing onayı{countdownLabel} ·{" "}
              </span>
              <span className="font-mono">
                {approval.from_agent} → {approval.to_agent}
              </span>{" "}
              · {approval.summary}
              <span className="ml-2 break-words text-outline">{approval.reason}</span>
            </div>
            <div className="relative z-[56] flex shrink-0 flex-wrap items-center gap-1.5 pointer-events-auto">
              {approval.kind === "agent_switch" ? (
                <button
                  type="button"
                  data-task-id={approval.task_id}
                  onClick={() => void resolveApproval("approve_local", approval.task_id)}
                  className={`${BANNER_BTN} border border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright`}
                >
                  Yerel modele geç
                </button>
              ) : null}
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("approve", approval.task_id)}
                className={`${BANNER_BTN} bg-primary-container font-semibold text-on-primary-container`}
              >
                Onayla
              </button>
              <button
                type="button"
                data-task-id={approval.task_id}
                onClick={() => void resolveApproval("deny", approval.task_id)}
                className={`${BANNER_BTN} border border-error bg-error-container text-on-error-container`}
              >
                Reddet
              </button>
            </div>
          </div>
        </div>
      ) : indexing ? (
        <div
          ref={attachBannerRef}
          className={`${bannerPos} border-b border-outline-variant bg-surface-container-low py-2 px-4`}
        >
          <div className="flex min-w-0 flex-wrap items-center gap-2 px-4 font-body text-body text-on-surface">
            <span
              className="h-3.5 w-3.5 shrink-0 animate-spin rounded-full border-2 border-transparent border-t-primary"
              aria-hidden
            />
            <span className="shrink-0 font-semibold tracking-label uppercase">Scanning...</span>
            <span className="whitespace-normal break-words text-outline">
              memory_bridge · index_workspace
            </span>
          </div>
        </div>
      ) : indexNotice ? (
        <div
          ref={attachBannerRef}
          className={`${bannerPos} border-b py-2 px-4 ${
            indexNotice.tone === "error"
              ? "border-error-container bg-error-container/20"
              : "border-secondary-container bg-secondary-container/30"
          }`}
        >
          <div
            className={`whitespace-normal break-words px-4 font-body text-body font-semibold leading-normal ${
              indexNotice.tone === "error" ? "text-error-dim" : "text-secondary-dim"
            }`}
          >
            {indexNotice.text}
          </div>
        </div>
      ) : approvalError ? (
        <div
          ref={attachBannerRef}
          className={`${bannerPos} border-b border-error-container bg-error-container/20 py-2 px-4`}
          role="status"
        >
          <div className="whitespace-normal break-words px-4 font-body text-body font-semibold leading-normal text-error-dim">
            {approvalError}
          </div>
        </div>
      ) : layaBanner && decisionGate ? (
        <div
          ref={attachBannerRef}
          className={`${bannerPos} border-b py-2 px-4 ${
            decisionGate.phase === "failed"
              ? "border-error-container bg-error-container"
              : decisionGate.phase === "available"
                ? "border-secondary-container bg-secondary-container"
                : "border-outline-variant bg-surface-container-low"
          }`}
        >
          <div className="flex min-w-0 flex-wrap items-start justify-between gap-3 px-4 font-body text-body">
            <div className="min-w-0 flex-1 whitespace-normal break-words leading-normal">
              <span
                className={`font-semibold tracking-label uppercase ${
                  decisionGate.phase === "failed" ? "text-on-error-container" : "text-on-surface"
                }`}
              >
                {decisionGate.title} ·{" "}
              </span>
              <span className={decisionGate.phase === "failed" ? "text-on-error-container" : "text-on-surface"}>
                {decisionGate.message}
                {decisionGate.detail ? ` · ${decisionGate.detail}` : ""}
              </span>
            </div>
            {decisionGate.phase === "available" ? (
              <div className="flex shrink-0 flex-wrap items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => void enableLaya()}
                  className={`${BANNER_BTN} bg-primary-container font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed`}
                >
                  Laya&apos;ya geç
                </button>
                <button
                  type="button"
                  onClick={() => void declineLaya()}
                  className={`${BANNER_BTN} border border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright`}
                >
                  Şimdilik LMR
                </button>
              </div>
            ) : decisionGate.phase === "failed" ? (
              <div className="flex shrink-0 flex-wrap items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => void enableLaya()}
                  className={`${BANNER_BTN} bg-primary-container font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed`}
                >
                  Yeniden dene
                </button>
                <button
                  type="button"
                  onClick={() => setLayaDismissed(true)}
                  className={`${BANNER_BTN} border border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright`}
                >
                  Gizle
                </button>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => setLayaDismissed(true)}
                className={`${BANNER_BTN} shrink-0 border border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright`}
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
            <span className="truncate font-headline text-panel font-bold tracking-tight uppercase">
              Agent Lounge OS
            </span>
            <span className="hidden shrink-0 rounded border border-outline-variant bg-surface-container-high px-1 py-0.5 font-body text-meta tracking-label text-primary uppercase 2xl:inline">
              KERNEL
            </span>
          </div>
          <div className="hidden min-w-0 items-center gap-1.5 overflow-hidden font-body text-body text-on-surface-variant 2xl:flex">
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
            <p className="min-w-0 flex-1 truncate font-body text-body text-on-surface-variant">
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
                className="w-full min-w-0 rounded-lg border border-outline-variant bg-surface-container-low py-1.5 pr-3 pl-8 font-body text-body text-on-surface placeholder:text-outline focus:border-primary focus:outline-none"
                placeholder="Filter subjects, repos, experiences"
              />
            </label>
          )}
        </div>

        <div className="flex min-w-0 shrink-0 items-center gap-1.5">
          {onboarding ? null : (
            <>
              <button
                type="button"
                onClick={() => setOpenCommandPalette(true)}
                className="flex shrink-0 items-center rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 hover:border-primary"
                title="Command Palette"
                aria-label="Open command palette"
              >
                <kbd className="font-mono text-meta text-on-surface-variant">{shortcutLabel}</kbd>
              </button>
              <ModelSelect
                model={model}
                models={models}
                lmrUp={report?.ollama.running === true}
                kernel={kernel}
                onChange={(next) => void applyModel(next)}
              />
              <div
                className={`hidden items-center gap-1 rounded border px-2 py-0.5 font-mono text-body 2xl:flex ${
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
                className={`flex min-w-0 max-w-[7.5rem] items-center gap-1 rounded border px-2 py-0.5 font-mono text-body xl:max-w-[14rem] ${
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
              <div className="tnum hidden shrink-0 text-body text-on-surface-variant 2xl:block">{clock} UTC+3</div>
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
          <div className="relative shrink-0" ref={alertRef}>
            <button
              type="button"
              className="rounded-lg p-1 text-on-surface-variant hover:bg-surface-container-high hover:text-on-surface"
              title="Alert history · critical security/quota"
              aria-label="Alert history"
              aria-expanded={alertOpen}
              aria-haspopup="dialog"
              onClick={() => setAlertOpen((open) => !open)}
            >
              <Icon name="bell" className="h-[15px] w-[15px]" />
            </button>
            {alertOpen ? (
              <div
                data-qa="alert-history"
                role="dialog"
                aria-label="Alert history"
                className="absolute top-full right-0 z-50 mt-1 w-72 rounded-lg border border-outline-variant bg-surface-container shadow-lg"
              >
                <div className="border-b border-outline-variant px-3 py-2 font-body text-meta font-semibold tracking-label text-on-surface uppercase">
                  Alert history
                </div>
                {criticalAlerts.length === 0 ? (
                  <p className="px-3 py-4 font-body text-body text-on-surface-variant">Uyarı yok</p>
                ) : (
                  <ul className="max-h-64 divide-y divide-outline-variant/40 overflow-auto py-1">
                    {criticalAlerts.map((alert) => (
                      <li key={alert.id} className="px-3 py-2">
                        <div className="font-mono text-meta font-semibold tracking-wider text-error-dim uppercase">
                          {alert.kind}
                        </div>
                        <div className="mt-0.5 font-body text-body text-on-surface">{alert.message}</div>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ) : null}
          </div>
        </div>
      </header>

      {onboarding ? null : (
      <aside
        data-qa="sidebar"
        className="fixed top-12 bottom-0 left-0 z-30 flex w-[var(--sidebar-w)] flex-col justify-between overflow-y-auto border-r border-outline-variant bg-surface-container-low px-2 py-3"
      >
        <div className="space-y-4">
          <div className="border-b border-outline-variant/60 px-2 pb-2">
            <div className="flex min-w-0 items-center gap-2">
              <BrandMark size={20} />
              <div className="min-w-0">
                <div className="truncate font-headline text-panel font-black tracking-label text-on-surface uppercase">
                  AL-OS CORE
                </div>
                <div className="font-mono text-meta text-on-surface-variant">
                  v0.1.0 · {serviceDegraded ? "degraded" : kernel}
                </div>
              </div>
            </div>
          </div>
          <nav className="space-y-0.5 font-label text-body">
            {NAV.map((item) => {
              const active =
                pathname === item.href || (item.id === "dashboard" && pathname === "/");
              return (
                <Link
                  key={item.id}
                  href={item.href}
                  className={`flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-body ${
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
              className="rounded border border-error-container/70 bg-error-container/15 px-2 py-1.5 font-mono text-meta text-error-dim"
              role="status"
            >
              <div className="font-bold tracking-wider uppercase">Service Degraded</div>
              <div className="mt-0.5 truncate text-on-surface-variant">
                {degradedNames.join(" · ")} down
              </div>
            </div>
          ) : null}
          <div className="space-y-1.5 rounded border border-outline-variant/40 bg-surface-container-lowest/60 p-2">
            <div className="mb-1 font-mono text-meta tracking-wider text-outline uppercase">Active daemons</div>
            {(
              [
                { name: "LMR", health: report?.ollama },
                { name: "NATS", health: report?.nats },
                { name: "Memory Bridge", health: report?.memory },
              ] as { name: string; health: ServiceHealth | undefined }[]
            ).map((daemon) => (
              <DaemonRow
                key={daemon.name}
                name={daemon.name}
                health={daemon.health}
                checked={daemonsChecked}
                onRestart={() => void restartServices()}
                restarting={restarting}
              />
            ))}
            <div
              className="flex items-center justify-between font-mono text-body"
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
        </div>
      </aside>
      )}

      <main
        className={`mt-12 flex h-[calc(100vh-3rem)] min-h-0 min-w-0 flex-1 flex-col gap-3 overflow-x-hidden overflow-y-auto bg-surface p-3.5 ${
          onboarding
            ? "ml-0 w-full"
            : "ml-[var(--sidebar-w)] w-[calc(100%-var(--sidebar-w))]"
        }`}
        style={bannerHeight > 0 ? { paddingTop: `calc(0.875rem + ${bannerHeight}px)` } : undefined}
      >
        <div className="flex min-h-0 min-w-0 w-full flex-1 flex-col">{children}</div>
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
      className="flex min-w-0 max-w-[11rem] items-center gap-1.5 rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 font-mono text-body text-on-surface-variant xl:max-w-[18rem]"
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
    return "Disconnected";
  }
  if (status.phase === "ready") {
    if (latencyMs != null && Number.isFinite(latencyMs)) {
      return `${latencyMs.toFixed(1)}ms`;
    }
    return status.device || "Running";
  }
  if (status.phase === "loading") {
    return "Checking…";
  }
  if (status.phase === "available") {
    return "önerildi";
  }
  return "Disconnected";
}

type AlertRow = { id: string; kind: string; message: string };

function buildCriticalAlerts(input: {
  quotas: ToolQuota[];
  amberAlert: boolean;
  amberTools: string[];
  approval: ApprovalRequest | null;
  degradedNames: string[];
}): AlertRow[] {
  const rows: AlertRow[] = [];
  if (input.approval && isSecurityApproval(input.approval.kind)) {
    rows.push({
      id: `sec:${input.approval.task_id}`,
      kind: "security",
      message: input.approval.summary || input.approval.reason,
    });
  }
  if (input.approval && isQuotaApproval(input.approval.kind)) {
    rows.push({
      id: `quota-approval:${input.approval.task_id}`,
      kind: "quota",
      message: input.approval.summary || input.approval.reason,
    });
  }
  for (const name of input.degradedNames) {
    rows.push({
      id: `svc:${name}`,
      kind: "service",
      message: `${name} disconnected`,
    });
  }
  const criticalQuotas = input.quotas.filter(
    (row) =>
      row.exhausted ||
      row.tone === "amber" ||
      row.tone === "warn" ||
      (row.percent != null && row.percent >= 80),
  );
  for (const row of criticalQuotas) {
    rows.push({
      id: `q:${row.id}`,
      kind: "quota",
      message: `${row.tool} · ${row.label}${row.percent != null ? ` · ${Math.round(row.percent)}%` : ""}`,
    });
  }
  if (rows.length === 0 && input.amberAlert) {
    rows.push({
      id: "amber",
      kind: "quota",
      message: `Amber alert · ${input.amberTools.join(", ") || "quota"}`,
    });
  }
  return rows.slice(0, 5);
}

function DaemonRow({
  name,
  health,
  checked,
  onRestart,
  restarting,
}: {
  name: string;
  health: ServiceHealth | undefined;
  checked: boolean;
  onRestart: () => void;
  restarting: boolean;
}) {
  const running = health?.running === true;
  const label = !checked
    ? "Checking services…"
    : daemonLabel(health, "Disconnected");
  return (
    <div className="space-y-0.5 font-mono text-body">
      <div className="flex items-center justify-between gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <Pip tone={daemonTone(health)} />
          <span className="truncate text-on-surface">{name}</span>
        </div>
        <span className={`tnum shrink-0 ${running ? "text-secondary" : "text-error"}`}>{label}</span>
      </div>
      {checked && !running ? (
        <button
          type="button"
          onClick={onRestart}
          disabled={restarting}
          className="ml-4 rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-meta text-on-surface hover:bg-surface-bright disabled:opacity-60"
        >
          {restarting ? "Restarting…" : "Restart Service"}
        </button>
      ) : null}
    </div>
  );
}
