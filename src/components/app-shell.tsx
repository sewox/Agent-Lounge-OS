"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { Icon } from "@/components/icons";
import { useLounge } from "@/components/lounge-provider";
import { daemonLabel, daemonTone, Pip } from "@/components/ui";

export type NavId = "stream" | "vault" | "health" | "fleet" | "telemetry" | "quotas" | "settings";

const NAV: { id: NavId; href: string; label: string; icon: string }[] = [
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
};

export function AppShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const {
    report,
    kernel,
    model,
    setModel,
    models,
    query,
    setQuery,
    clock,
    indexing,
    quotas,
    approval,
    applyModel,
    indexWorkspace,
    resolveApproval,
  } = useLounge();

  const crumb = TITLES[pathname] ?? "Lounge";
  const warnQuota = quotas.find((row) => (row.percent ?? 0) >= 70);
  const onboarding = pathname === "/onboarding";

  return (
    <div className="min-h-screen bg-surface text-on-surface">
      {approval ? (
        <div className="fixed inset-x-0 top-12 z-50 border-b border-error-container bg-error-container/20 px-[220px] py-2">
          <div className="flex flex-wrap items-center justify-between gap-2 px-4 font-mono text-[11px]">
            <div className="text-on-surface">
              <span className="font-bold uppercase text-error-dim">Routing onayı · </span>
              {approval.from_agent} → {approval.to_agent} · {approval.summary}
              <span className="ml-2 text-outline">{approval.reason}</span>
            </div>
            <div className="flex items-center gap-1.5">
              {approval.kind === "quota_local_fallback" || approval.kind === "agent_switch" ? (
                <button
                  type="button"
                  onClick={() => void resolveApproval("approve_local")}
                  className="rounded border border-outline-variant bg-surface-container-high px-2 py-1 text-on-surface hover:bg-surface-bright"
                >
                  Yerel modele geç
                </button>
              ) : null}
              <button
                type="button"
                onClick={() => void resolveApproval("approve")}
                className="rounded bg-primary-container px-2 py-1 font-semibold text-on-primary-container"
              >
                Onayla
              </button>
              <button
                type="button"
                onClick={() => void resolveApproval("deny")}
                className="rounded border border-error bg-error-container px-2 py-1 text-on-error-container"
              >
                Reddet
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <header className="fixed top-0 left-0 z-40 flex h-12 w-full items-center justify-between border-b border-outline-variant bg-surface px-3">
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2 border-r border-outline-variant pr-3">
            <span className="font-headline font-mono text-xs font-bold tracking-tight uppercase">
              Agent Lounge OS
            </span>
            <span className="rounded border border-outline-variant bg-surface-container-high px-1 py-0.5 font-mono text-[9px] text-primary uppercase">
              KERNEL
            </span>
          </div>
          <div className="flex items-center gap-1.5 font-mono text-xs text-on-surface-variant">
            <span>Overview</span>
            <span className="text-outline">/</span>
            <span className="font-medium text-on-surface">{crumb}</span>
          </div>
        </div>

        {onboarding ? (
          <div className="mx-6 flex-1 font-mono text-[11px] text-on-surface-variant">
            Yerel araçları tarayıp Lounge’a bağlayın
          </div>
        ) : (
          <div className="mx-6 flex max-w-lg flex-1 items-center gap-4">
            <label className="relative w-full">
              <span className="pointer-events-none absolute inset-y-0 left-2.5 flex items-center text-outline">
                <Icon name="search" />
              </span>
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                className="w-full rounded-lg border border-outline-variant bg-surface-container-low py-1 pr-12 pl-8 font-body text-xs text-on-surface placeholder:text-outline focus:border-primary focus:outline-none"
                placeholder="Filter subjects, repos, experiences"
              />
              <span className="pointer-events-none absolute inset-y-0 right-2 flex items-center">
                <kbd className="rounded border border-outline-variant bg-surface-container-high px-1.5 py-0.5 font-mono text-[10px] text-on-surface-variant">
                  ⌘K
                </kbd>
              </span>
            </label>
            <div className="hidden shrink-0 items-center gap-3 font-mono text-[11px] xl:flex">
              <span className="border-b border-primary px-1 py-0.5 font-medium text-primary">Cluster us-east</span>
              <span className="text-on-surface-variant">NATS v2.10</span>
              <span className="text-on-surface-variant">Latency 4ms</span>
            </div>
          </div>
        )}

        <div className="flex items-center gap-2">
          {onboarding ? null : (
          <div className="mr-1 flex items-center gap-1.5 font-mono text-[11px]">
            <label className="flex items-center gap-1.5 rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 text-on-surface-variant">
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-primary" />
              <input
                list="ollama-models"
                value={model}
                onChange={(event) => setModel(event.target.value)}
                onBlur={() => void applyModel()}
                className="w-28 bg-transparent text-on-surface outline-none"
              />
              <datalist id="ollama-models">
                <option value="llama3.1:8b" />
                {models.map((item) => (
                  <option key={item} value={item} />
                ))}
              </datalist>
            </label>
            <div className="flex items-center gap-1 rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 text-primary">
              <span className="h-1.5 w-1.5 rounded-full bg-secondary" />
              <span>LOCAL</span>
            </div>
            <Link
              href="/quotas"
              className="flex items-center gap-1 rounded border border-error-container bg-error-container/20 px-2 py-0.5 font-mono text-[11px] text-error-dim"
            >
              <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-error" />
              <span>
                {warnQuota
                  ? `QUOTA ${Math.round(warnQuota.percent ?? 0)}% ${warnQuota.id}`
                  : "QUOTA OK"}
              </span>
            </Link>
            <div className="tnum hidden text-on-surface-variant sm:block">{clock} UTC+3</div>
          </div>
          )}
          {onboarding ? null : (
            <>
              <button
                type="button"
                className="flex items-center gap-1 rounded-lg border border-outline-variant bg-surface-container-high px-2.5 py-1 text-xs font-medium text-on-surface hover:bg-surface-bright"
              >
                <Icon name="tune" />
                <span className="hidden sm:inline">Quick Filter</span>
              </button>
              <button
                type="button"
                onClick={() => void indexWorkspace()}
                className="flex items-center gap-1 rounded-lg bg-primary-container px-2.5 py-1 text-xs font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed"
              >
                <Icon name="terminal" />
                <span>{indexing ? "Indexing…" : "Index Workspace"}</span>
              </button>
            </>
          )}
          <span className="rounded-lg p-1 text-on-surface-variant" title={`kernel · ${kernel}`}>
            <Icon name="bell" className="h-[15px] w-[15px]" />
          </span>
        </div>
      </header>

      {onboarding ? null : (
      <aside className="fixed top-12 bottom-0 left-0 z-30 flex w-[220px] flex-col justify-between border-r border-outline-variant bg-surface-container-low px-2 py-3">
        <div className="space-y-4">
          <div className="flex items-center justify-between border-b border-outline-variant/60 px-2 pb-2">
            <div>
              <div className="font-headline text-xs font-black tracking-wider text-on-surface uppercase">
                AL-OS CORE
              </div>
              <div className="font-mono text-[10px] text-on-surface-variant">v0.1.0 · {kernel}</div>
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
              const active = pathname === item.href || (item.href === "/stream" && pathname === "/");
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
          <div className="space-y-1.5 rounded border border-outline-variant/40 bg-surface-container-lowest/60 p-2">
            <div className="mb-1 font-mono text-[10px] tracking-wider text-outline uppercase">Active daemons</div>
            {[
              { name: "Ollama", health: report?.ollama, fallback: "—" },
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

      <main className={`mt-12 min-h-[calc(100vh-48px)] space-y-3 bg-surface p-3.5 ${onboarding ? "ml-0" : "ml-[220px]"} ${approval ? "pt-14" : ""}`}>
        {children}
      </main>
    </div>
  );
}
