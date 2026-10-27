"use client";

import { EventStreamPanel, OverviewKpis } from "@/components/panels";
import { JournalPanel } from "@/components/journal-panel";

export default function StreamPage() {
  return (
    <div className="flex h-full min-h-0 w-full flex-col gap-3">
      <div className="w-full shrink-0">
        <OverviewKpis />
      </div>
      <div className="w-full shrink-0">
        <JournalPanel />
      </div>
      <div className="min-h-0 w-full flex-1">
        <EventStreamPanel />
      </div>
    </div>
  );
}
