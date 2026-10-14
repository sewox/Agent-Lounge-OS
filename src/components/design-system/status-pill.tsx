"use client";

import type { ReactNode } from "react";

export type StatusTone = "ok" | "warn" | "error" | "halted" | "neutral";

export type StatusPillProps = {
  tone: StatusTone;
  children: ReactNode;
  className?: string;
};

function toneClasses(tone: StatusTone): string {
  switch (tone) {
    case "ok":
      return "border-secondary/50 bg-secondary-container/40 text-secondary-dim";
    case "warn":
      return "border-tertiary/40 bg-surface-container-highest text-tertiary";
    case "error":
      return "border-error bg-error-container text-on-error-container";
    case "halted":
      return "border-error ds-halted-hatch text-on-error-container";
    case "neutral":
    default:
      return "border-outline-variant bg-surface-container-high text-on-surface-variant";
  }
}

/**
 * Compact status chip. `halted` uses the hatched deliberately-stopped language
 * (distinct from solid `error`).
 */
export function StatusPill({ tone, children, className = "" }: StatusPillProps) {
  return (
    <span
      data-qa="status-pill"
      data-tone={tone}
      className={`inline-flex max-w-full items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-meta font-medium uppercase tracking-label ${toneClasses(tone)} ${className}`}
    >
      {children}
    </span>
  );
}
