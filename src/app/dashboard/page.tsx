"use client";

import Link from "next/link";
import { CollapsiblePanel } from "@/components/collapsible-panel";
import { EventStreamPanel, OverviewKpis, QuotaMiniCard, VaultPanel } from "@/components/panels";

export default function DashboardPage() {
  return (
    <div className="flex min-h-0 flex-col gap-3 pb-4">
      <div className="shrink-0">
        <OverviewKpis />
      </div>
      <div className="grid grid-cols-1 gap-3 2xl:grid-cols-[minmax(0,1.45fr)_minmax(0,1fr)] 2xl:items-start">
        <CollapsiblePanel
          id="dash-event-stream"
          title="NATS Event Stream"
          bodyClassName="flex min-h-[var(--stream-min-h)] max-h-[var(--panel-max-h)] h-[var(--stream-min-h)] 2xl:h-[var(--panel-max-h)] flex-col"
        >
          <EventStreamPanel embedded />
        </CollapsiblePanel>
        <div className="flex min-w-0 flex-col gap-3">
          <CollapsiblePanel
            id="dash-vault"
            title="Semantic Map + Experiences"
            bodyClassName="flex min-h-[var(--panel-min-h)] max-h-[var(--panel-max-h)] h-[var(--panel-min-h)] 2xl:h-[var(--panel-max-h)] flex-col"
          >
            <VaultPanel embedded />
          </CollapsiblePanel>
          <CollapsiblePanel
            id="dash-quota"
            title="Critical Quotas"
            trailer={
              <Link
                href="/quotas"
                className="font-body text-meta font-medium text-primary hover:underline"
              >
                Tüm kotalar →
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
