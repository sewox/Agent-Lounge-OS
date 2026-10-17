"use client";

import { createElement, type ReactNode } from "react";

export type StatsStripItem = {
  /** Stable id / filter key written to FilterBar or list filter. */
  id: string;
  label: string;
  value: number | string;
  /** Optional hint under the number. */
  hint?: string;
};

export type StatsStripProps = {
  items: StatsStripItem[];
  /** Active filter id; `null` / `undefined` = none (or "all" if provided as an item). */
  activeId?: string | null;
  onFilterChange: (id: string) => void;
  className?: string;
  /** Leading slot (e.g. title). */
  leading?: ReactNode;
};

export function StatsStrip({
  items,
  activeId = null,
  onFilterChange,
  className = "",
  leading,
}: StatsStripProps) {
  return createElement(
    "div",
    {
      "data-qa": "stats-strip",
      role: "toolbar",
      "aria-label": "Category filters",
      className: `flex w-full min-w-0 flex-wrap items-stretch gap-2 ${className}`,
    },
    leading,
    ...items.map((item) => {
      const pressed = activeId === item.id;
      const valueText =
        typeof item.value === "number" ? item.value.toLocaleString("en-US") : item.value;
      // Accessible name comes from visible label+value (WCAG 2.5.3). aria-pressed
      // conveys filter state; title is a non-name hint only.
      const titleHint = pressed ? "Active filter" : "Set filter";
      return createElement(
        "button",
        {
          key: item.id,
          type: "button",
          "data-qa": "stats-strip-item",
          "data-stat-id": item.id,
          "data-active": pressed ? "true" : "false",
          title: titleHint,
          "aria-pressed": pressed,
          onClick: () => onFilterChange(item.id),
          className: `flex min-h-14 min-w-[6.5rem] flex-1 flex-col justify-between rounded-lg border px-3 py-2 text-left transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary ${
            pressed
              ? "border-primary bg-primary-container text-on-primary-container ring-1 ring-primary/40"
              : "border-outline-variant bg-surface-container text-on-surface hover:bg-surface-container-high"
          }`,
        },
        createElement(
          "span",
          {
            lang: "en",
            className: `font-label text-meta font-medium tracking-label uppercase ${
              pressed ? "text-on-primary-container" : "text-on-surface-variant"
            }`,
          },
          item.label,
        ),
        createElement(
          "span",
          { className: "tnum font-headline text-kpi font-semibold tabular-nums" },
          valueText,
        ),
        item.hint
          ? createElement("span", { className: "font-body text-meta text-outline" }, item.hint)
          : null,
      );
    }),
  );
}
