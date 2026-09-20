"use client";

import { EventStreamPanel, OverviewKpis, QuotaPanel, VaultPanel } from "@/components/panels";

export default function DashboardPage() {
  return (
    <>
      <OverviewKpis />
      <EventStreamPanel />
      <VaultPanel />
      <QuotaPanel />
    </>
  );
}
