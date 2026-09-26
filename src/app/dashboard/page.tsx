"use client";

import Link from "next/link";
import { CollapsiblePanel } from "@/components/collapsible-panel";
import { EventStreamPanel, OverviewKpis, QuotaMiniCard, VaultPanel } from "@/components/panels";

export default function DashboardPage() {
  return (
    <div className="flex w-full min-h-0 flex-col gap-3 pb-4">
      <div className="w-full shrink-0">
        <OverviewKpis />
      </div>
      {/* xl (1280): side-by-side so embedded Vault is in the first fold (DB-04). */}
      <div className="grid w-full grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)] xl:items-start">
        <CollapsiblePanel
          id="dash-event-stream"
          title="NATS Event Stream"
          className="w-full"
          bodyClassName="flex min-h-[16rem] max-h-[var(--panel-max-h)] h-[min(20rem,var(--stream-min-h))] xl:h-[var(--panel-max-h)] flex-col"
        >
          <EventStreamPanel embedded />
        </CollapsiblePanel>
        <div className="flex w-full min-w-0 flex-col gap-3">
          <CollapsiblePanel
            id="dash-vault"
            title="Semantic Map + Experiences"
            className="w-full"
            bodyClassName="flex min-h-[var(--panel-min-h)] max-h-[var(--panel-max-h)] h-[var(--panel-min-h)] xl:h-[var(--panel-max-h)] flex-col"
          >
            <VaultPanel embedded />
          </CollapsiblePanel>
          <CollapsiblePanel
            id="dash-quota"
            title="Critical Quotas"
            className="w-full"
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
