"use client";

import type { ReactNode } from "react";

export type StickyActionBarProps = {
  children: ReactNode;
  className?: string;
  /** Accessible name for the action region. */
  label?: string;
  /** Leading status text (e.g. "3 selected"). */
  leading?: ReactNode;
};

/**
 * Bottom sticky actions for multi-select surfaces (Vault Review bulk bar).
 * Stays visible while the list scrolls; does not invent min-height fillers.
 */
export function StickyActionBar({
  children,
  className = "",
  label = "Bulk actions",
  leading,
}: StickyActionBarProps) {
  return (
    <div
      data-qa="sticky-action-bar"
      role="region"
      aria-label={label}
      className={`sticky bottom-0 z-[1] flex w-full min-w-0 flex-wrap items-center gap-2 border-t border-outline-variant bg-surface-container-high/95 px-3 py-2 backdrop-blur-sm ${className}`}
    >
      {leading ? (
        <div className="min-w-0 flex-1 font-body text-body text-on-surface-variant">{leading}</div>
      ) : (
        <div className="min-w-0 flex-1" />
      )}
      <div className="flex flex-wrap items-center justify-end gap-2">{children}</div>
    </div>
  );
}
