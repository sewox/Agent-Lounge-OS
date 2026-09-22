"use client";

import { EventStreamPanel, OverviewKpis, QuotaPanel, VaultPanel } from "@/components/panels";

export default function DashboardPage() {
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="shrink-0">
        <OverviewKpis />
      </div>
      <div className="grid min-h-0 flex-1 grid-rows-3 gap-3">
        <EventStreamPanel />
        <VaultPanel />
        <QuotaPanel />
      </div>
    </div>
  );
}
