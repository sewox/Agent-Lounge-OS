"use client";

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { EventStreamPanel, OverviewKpis, QuotaPanel, VaultPanel } from "@/components/panels";
import { useLounge } from "@/components/lounge-provider";
import { BUS_UI_EVENT, isTauri, type LoungeMessage } from "@/lib/lounge";

export default function DashboardPage() {
  const { ingestBusMessage } = useLounge();

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    const boot = window.setTimeout(() => {
      void listen<LoungeMessage>(BUS_UI_EVENT, (event) => {
        if (!cancelled) {
          ingestBusMessage(event.payload);
        }
      }).then((fn) => {
        if (cancelled) {
          void fn();
          return;
        }
        unlisten = fn;
      });
    }, 0);
    return () => {
      cancelled = true;
      window.clearTimeout(boot);
      if (unlisten) {
        void unlisten();
      }
    };
  }, [ingestBusMessage]);

  return (
    <>
      <OverviewKpis />
      <EventStreamPanel />
      <VaultPanel />
      <QuotaPanel />
    </>
  );
}
