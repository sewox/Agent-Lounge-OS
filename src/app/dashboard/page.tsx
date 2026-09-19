"use client";

import { EventStreamPanel, OverviewKpis } from "@/components/panels";

export default function DashboardPage() {
  return (
    <>
      <OverviewKpis />
      <EventStreamPanel />
    </>
  );
}
