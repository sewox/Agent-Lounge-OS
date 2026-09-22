"use client";

import { EventStreamPanel, OverviewKpis } from "@/components/panels";

export default function StreamPage() {
  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="shrink-0">
        <OverviewKpis />
      </div>
      <div className="min-h-0 flex-1">
        <EventStreamPanel />
      </div>
    </div>
  );
}
