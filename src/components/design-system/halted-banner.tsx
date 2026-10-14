"use client";

import { useEffect, useState, type ReactNode } from "react";

export type HaltedBannerProps = {
  /** Short title — defaults to "Halted". */
  title?: string;
  children?: ReactNode;
  className?: string;
};

function readPrefersReducedMotion(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return false;
  }
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Red banner + hatched field meaning "deliberately stopped" (not error/crash).
 * Visual-only — Halt control logic is PR-Control-1.
 */
export function HaltedBanner({
  title = "Halted",
  children,
  className = "",
}: HaltedBannerProps) {
  const [reducedMotion, setReducedMotion] = useState(readPrefersReducedMotion);

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReducedMotion(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  return (
    <div
      role="status"
      aria-live="polite"
      data-qa="halted-banner"
      data-reduced-motion={reducedMotion ? "true" : "false"}
      className={`overflow-hidden rounded-lg border border-error ${className}`}
    >
      <div className="flex items-center gap-2 bg-error-container px-3 py-2 font-body text-body font-semibold text-on-error-container">
        <span
          aria-hidden
          className="inline-block h-2 w-2 rounded-full bg-error"
        />
        <span className="tracking-label uppercase">{title}</span>
        <span className="font-normal text-on-error-container/90">
          Deliberately stopped — not an error
        </span>
      </div>
      <div
        data-qa="halted-hatch"
        className={`min-h-10 px-3 py-2 font-body text-body text-on-surface ${
          reducedMotion ? "ds-halted-hatch" : "ds-halted-hatch ds-halted-hatch-animate"
        }`}
      >
        {children ?? (
          <p className="text-on-surface-variant">
            Operator Halt is active. Resume from Controls when ready.
          </p>
        )}
      </div>
    </div>
  );
}
