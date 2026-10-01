"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { DeadSymbolsPanel } from "@/components/dead-symbols-panel";
import { HealthPanel } from "@/components/panels";
import { deadSymbolStrings as s } from "@/lib/strings/dead-symbols";

type HealthTab = "health" | "dead" | "ignore";

function parseTab(raw: string | null): HealthTab {
  if (raw === "dead" || raw === "ignore") {
    return raw;
  }
  return "health";
}

export default function HealthPageClient() {
  const params = useSearchParams();
  const tab = parseTab(params.get("tab"));
  const project = params.get("project");

  return (
    <div className="flex h-full min-h-0 w-full flex-col gap-2">
      <nav
        aria-label="Health sections"
        className="flex shrink-0 flex-wrap items-center gap-1 rounded-lg border border-outline-variant bg-surface-container-low p-1"
      >
        <HealthTabLink href="/health?tab=health" active={tab === "health"} label={s.healthTab} />
        <HealthTabLink href="/health?tab=dead" active={tab === "dead"} label={s.deadTab} />
        <HealthTabLink href="/health?tab=ignore" active={tab === "ignore"} label={s.ignoreTab} />
      </nav>
      <div className="min-h-0 flex-1">
        {tab === "health" ? (
          <HealthPanel />
        ) : tab === "ignore" ? (
          <DeadSymbolsPanel mode="ignored" projectFilter={project} />
        ) : (
          <DeadSymbolsPanel mode="dead" projectFilter={project} />
        )}
      </div>
    </div>
  );
}

function HealthTabLink({
  href,
  active,
  label,
}: {
  href: string;
  active: boolean;
  label: string;
}) {
  return (
    <Link
      href={href}
      className={`min-h-8 rounded-md px-2.5 py-1 font-body text-body font-medium ${
        active
          ? "bg-primary-container text-on-primary-container"
          : "text-on-surface-variant hover:bg-surface-container-high"
      }`}
    >
      {label}
    </Link>
  );
}
