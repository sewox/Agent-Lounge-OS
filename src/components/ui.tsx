import type { ReactNode } from "react";
import { natsEventTone, type ExperienceOutcome, type NatsEvent, type NatsTone, type ServiceHealth } from "@/lib/lounge";

export function Pip({ live = false, tone = "ok" }: { live?: boolean; tone?: "ok" | "warn" | "down" | "primary" }) {
  const color =
    tone === "down"
      ? "bg-error"
      : tone === "warn"
        ? "bg-tertiary"
        : tone === "primary"
          ? "bg-primary"
          : "bg-secondary-fixed-dim";
  return (
    <span
      className={`inline-block h-1.5 w-1.5 rounded-full ${color} ${live ? "animate-pulse ring-2 ring-primary/20" : "ring-2 ring-secondary/20"}`}
    />
  );
}

export function daemonTone(health: ServiceHealth | undefined): "ok" | "warn" | "down" {
  if (!health) {
    return "warn";
  }
  return health.running ? "ok" : "down";
}

export function daemonLabel(health: ServiceHealth | undefined, fallback: string): string {
  if (!health) {
    return fallback;
  }
  return health.running ? "up" : "down";
}

export function LatencySparkline({ values }: { values: number[] }) {
  if (values.length < 2) {
    return null;
  }
  const width = 56;
  const height = 16;
  const max = Math.max(...values, 0.1);
  const points = values
    .map((value, index) => {
      const x = (index / (values.length - 1)) * width;
      const y = height - (value / max) * (height - 2) - 1;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      className="text-primary"
      aria-hidden
    >
      <polyline fill="none" stroke="currentColor" strokeWidth="1.5" points={points} />
    </svg>
  );
}

export function outcomeClass(outcome: ExperienceOutcome): string {
  if (outcome === "success") {
    return "bg-secondary-container/60 text-secondary-dim border-secondary-container";
  }
  if (outcome === "partial") {
    return "bg-surface-container-highest text-tertiary border-outline-variant";
  }
  return "bg-error-container text-on-error-container border-error";
}

export function eventStateClass(state: NatsEvent["state"], subject = ""): string {
  return eventToneClass(natsEventTone(subject, state));
}

export function eventToneClass(tone: NatsTone): string {
  if (tone === "success") {
    return "bg-secondary-container/40 text-secondary-dim border-secondary-container";
  }
  if (tone === "error") {
    return "bg-error-container text-on-error-container border-error";
  }
  return "bg-primary-container/50 text-primary border-primary/40";
}

export function subjectClass(subject: string, selected: boolean): string {
  if (selected) {
    return "text-primary font-medium";
  }
  const tone = natsEventTone(subject);
  if (tone === "error") {
    return "text-error font-medium";
  }
  if (tone === "success") {
    return "text-secondary";
  }
  return "text-primary";
}

export function Kpi({
  label,
  value,
  hint,
  badge,
  valueClass = "text-on-surface",
  live = false,
}: {
  label: string;
  value: string;
  hint: string;
  badge: ReactNode;
  valueClass?: string;
  live?: boolean;
}) {
  return (
    <div className="flex flex-col justify-between rounded-lg border border-outline-variant bg-surface-container p-2.5">
      <div className="flex items-center justify-between text-on-surface-variant">
        <span className="font-mono text-[11px] tracking-wider uppercase">{label}</span>
      </div>
      <div className="mt-1 flex items-baseline justify-between">
        <div
          key={value}
          data-live={live || undefined}
          className={`kpi-tick tnum font-mono text-xl font-bold tracking-tight ${valueClass}`}
        >
          {value}
        </div>
        {badge}
      </div>
      <div className="mt-1 font-mono text-[10px] text-outline">{hint}</div>
    </div>
  );
}

export function Pager({
  page,
  pages,
  total,
  onPage,
}: {
  page: number;
  pages: number;
  total: number;
  onPage: (next: number) => void;
}) {
  const totalPages = Math.max(1, pages);
  const safe = Math.min(Math.max(0, page), totalPages - 1);
  return (
    <div className="flex items-center gap-2 font-mono text-[10px]" suppressHydrationWarning>
      <button
        type="button"
        disabled={safe <= 0}
        onClick={() => onPage(Math.max(0, safe - 1))}
        className="rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 text-on-surface disabled:cursor-not-allowed disabled:opacity-40"
      >
        Prev
      </button>
      <span className="tnum text-on-surface-variant">
        {safe + 1}/{totalPages}
        <span className="ml-1 text-outline">· {total}</span>
      </span>
      <button
        type="button"
        disabled={safe >= totalPages - 1}
        onClick={() => onPage(Math.min(totalPages - 1, safe + 1))}
        className="rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 text-on-surface disabled:cursor-not-allowed disabled:opacity-40"
      >
        Next
      </button>
    </div>
  );
}
