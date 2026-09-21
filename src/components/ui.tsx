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
  return health.running ? (health.started_by_us ? "12ms" : "4ms") : "down";
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
}: {
  label: string;
  value: string;
  hint: string;
  badge: ReactNode;
  valueClass?: string;
}) {
  return (
    <div className="flex flex-col justify-between rounded-lg border border-outline-variant bg-surface-container p-2.5">
      <div className="flex items-center justify-between text-on-surface-variant">
        <span className="font-mono text-[11px] tracking-wider uppercase">{label}</span>
      </div>
      <div className="mt-1 flex items-baseline justify-between">
        <div className={`tnum font-mono text-xl font-bold tracking-tight ${valueClass}`}>{value}</div>
        {badge}
      </div>
      <div className="mt-1 font-mono text-[10px] text-outline">{hint}</div>
    </div>
  );
}
