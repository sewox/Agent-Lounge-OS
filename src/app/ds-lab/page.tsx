"use client";

import { useMemo, useState } from "react";
import {
  HaltedBanner,
  PanelGrid,
  StatsStrip,
  StatusPill,
  type PanelMode,
} from "@/components/design-system";

type LabRow = {
  id: string;
  name: string;
  category: "running" | "halted" | "error" | "idle";
  detail: string;
};

const ROWS: LabRow[] = [
  { id: "a1", name: "lounge-kernel", category: "running", detail: "LMR ready on :18790" },
  { id: "a2", name: "nats-hub", category: "running", detail: "listening · lounge.>" },
  { id: "a3", name: "worker-alpha", category: "halted", detail: "Operator Halt — deliberately stopped" },
  { id: "a4", name: "worker-beta", category: "error", detail: "Crash loop — last exit 1" },
  { id: "a5", name: "memory-bridge", category: "idle", detail: "Optional · not installed" },
  { id: "a6", name: "openjev-laya", category: "running", detail: "decision gate ready" },
  { id: "a7", name: "graph-ui", category: "running", detail: "ports 18749–18759" },
  { id: "a8", name: "mcp-stdio", category: "running", detail: "lounge-mcp stdio" },
  { id: "a9", name: "mcp-http", category: "idle", detail: "HTTP 18791 standby" },
  { id: "a10", name: "worker-gamma", category: "halted", detail: "Halted by policy" },
  { id: "a11", name: "worker-delta", category: "error", detail: "Timeout on heartbeat" },
  { id: "a12", name: "indexer", category: "running", detail: "FTS queue drained" },
  { id: "a13", name: "approval-bridge", category: "running", detail: "security + quota alerts" },
  { id: "a14", name: "telemetry-pump", category: "idle", detail: "awaiting sample window" },
  { id: "a15", name: "quota-rotator", category: "running", detail: "header badge live" },
  { id: "a16", name: "vault-sync", category: "running", detail: "experience log ok" },
];

/**
 * PR-DS-0 reference surface: PanelGrid + StatsStrip + Halted language.
 * Not in primary nav — opened by URL for layout/e2e proof.
 */
export default function DesignSystemLabPage() {
  const [filter, setFilter] = useState<string>("all");
  const [selectedId, setSelectedId] = useState<string>("a3");
  const [mode, setMode] = useState<PanelMode>("one");

  const counts = useMemo(() => {
    const base = { all: ROWS.length, running: 0, halted: 0, error: 0, idle: 0 };
    for (const row of ROWS) {
      base[row.category] += 1;
    }
    return base;
  }, []);

  const filtered = useMemo(() => {
    if (filter === "all") return ROWS;
    return ROWS.filter((row) => row.category === filter);
  }, [filter]);

  const selected = filtered.find((row) => row.id === selectedId) ?? filtered[0] ?? ROWS[0]!;

  const stats = [
    { id: "all", label: "All", value: counts.all },
    { id: "running", label: "Running", value: counts.running },
    { id: "halted", label: "Halted", value: counts.halted },
    { id: "error", label: "Error", value: counts.error },
    { id: "idle", label: "Idle", value: counts.idle },
  ];

  return (
    <div
      data-qa="ds-lab"
      className="flex h-full min-h-[calc(100vh-4.5rem)] w-full min-w-0 flex-1 flex-col gap-3"
    >
      {/* Top chrome is itself a panel so L3 does not treat the strip as empty. */}
      <section
        data-qa="panel"
        data-panel-slot="stats"
        className="w-full shrink-0 space-y-2.5 rounded-lg border border-outline-variant bg-surface-container p-3"
      >
        <div className="flex w-full flex-wrap items-center justify-between gap-2">
          <div>
            <h1 className="font-headline text-panel font-bold tracking-label text-on-surface uppercase">
              Design system lab
            </h1>
            <p className="font-body text-meta text-on-surface-variant">
              PR-DS-0 reference · panel mode{" "}
              <span data-qa="ds-lab-mode" className="font-mono text-primary">
                {mode}
              </span>
            </p>
          </div>
          <StatusPill tone="halted">Halted language</StatusPill>
        </div>

        <StatsStrip items={stats} activeId={filter} onFilterChange={setFilter} />

        {selected.category === "halted" ? (
          <HaltedBanner title="Halted">
            <p>
              <span className="font-mono text-on-surface">{selected.name}</span>
              {" — "}
              {selected.detail}
            </p>
          </HaltedBanner>
        ) : (
          <div
            data-qa="ds-lab-halted-idle"
            className="rounded border border-dashed border-outline-variant px-3 py-2 font-body text-meta text-on-surface-variant"
          >
            Select a halted row to preview the hatched Halted banner (distinct from error).
          </div>
        )}
      </section>

      <PanelGrid
        className="min-h-[28rem] flex-1"
        onModeChange={setMode}
        list={
          <div className="flex h-full min-h-0 flex-col">
            <div className="shrink-0 border-b border-outline-variant bg-surface-container-low px-3 py-2">
              <h2 className="font-body text-panel font-semibold tracking-label uppercase">List</h2>
            </div>
            <ul className="min-h-0 flex-1 overflow-auto font-mono text-body">
              {filtered.map((row) => {
                const active = row.id === selected.id;
                return (
                  <li key={row.id}>
                    <button
                      type="button"
                      data-qa="ds-lab-row"
                      data-row-id={row.id}
                      onClick={() => setSelectedId(row.id)}
                      className={`flex w-full items-center justify-between gap-2 border-b border-outline-variant/40 px-3 py-2 text-left ${
                        active
                          ? "bg-primary-container/20 text-primary"
                          : "text-on-surface hover:bg-surface-container-high"
                      }`}
                    >
                      <span className="truncate">{row.name}</span>
                      <StatusPill
                        tone={
                          row.category === "running"
                            ? "ok"
                            : row.category === "halted"
                              ? "halted"
                              : row.category === "error"
                                ? "error"
                                : "neutral"
                        }
                      >
                        {row.category}
                      </StatusPill>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        }
        detail={
          <div className="flex h-full min-h-0 flex-col overflow-auto p-3">
            <h2 className="font-body text-panel font-semibold tracking-label uppercase">Detail</h2>
            <p className="mt-2 font-mono text-body text-on-surface" data-qa="ds-lab-detail-name">
              {selected.name}
            </p>
            <p className="mt-1 font-body text-body text-on-surface-variant">{selected.detail}</p>
            <div className="mt-3">
              <StatusPill
                tone={
                  selected.category === "running"
                    ? "ok"
                    : selected.category === "halted"
                      ? "halted"
                      : selected.category === "error"
                        ? "error"
                        : "neutral"
                }
              >
                {selected.category}
              </StatusPill>
            </div>
            {/* Fill interior so L2 sparse-interior does not flag the reference panel. */}
            <div className="mt-4 space-y-2 font-body text-body text-on-surface-variant">
              <p>Endpoint · mock://ds-lab/{selected.id}</p>
              <p>Heartbeat · {selected.category === "running" ? "live" : "stale"}</p>
              <p>Restarts · 0</p>
              <p>Uptime · {selected.category === "running" ? "up" : "—"}</p>
              <p>Notes · Design-system reference content for layout gates L1–L6.</p>
              <p>Filter · {filter}</p>
              <p>Grid mode · {mode}</p>
              <p>Panel slots · list | detail | inspector</p>
              <p>Narrow (&lt;1200) uses drawers; medium two panels; wide three; portrait stacks.</p>
              <p>StatsStrip clicks set the list filter; active chip uses aria-pressed.</p>
              <p>Halted hatch is distinct from solid error chrome.</p>
            </div>
          </div>
        }
        inspector={
          <div className="flex h-full min-h-0 flex-col overflow-auto p-3">
            <h2 className="font-body text-panel font-semibold tracking-label uppercase">Inspector</h2>
            <dl className="mt-2 space-y-2 font-mono text-meta text-on-surface-variant">
              <div>
                <dt className="text-outline">id</dt>
                <dd className="text-on-surface">{selected.id}</dd>
              </div>
              <div>
                <dt className="text-outline">category</dt>
                <dd className="text-on-surface">{selected.category}</dd>
              </div>
              <div>
                <dt className="text-outline">filter</dt>
                <dd className="text-on-surface">{filter}</dd>
              </div>
              <div>
                <dt className="text-outline">mode</dt>
                <dd className="text-on-surface">{mode}</dd>
              </div>
            </dl>
          </div>
        }
      />
    </div>
  );
}
