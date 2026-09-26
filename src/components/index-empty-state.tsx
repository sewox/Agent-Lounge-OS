"use client";

import { useLounge } from "@/components/lounge-provider";

type IndexEmptyStateProps = {
  /** Extra context under the title (e.g. Map / Health). */
  detail?: string;
  className?: string;
};

/** Honest empty state when index/map/health has no data (HM-01/HM-02 / K9). */
export function IndexEmptyState({
  detail = "No data found.",
  className = "",
}: IndexEmptyStateProps) {
  const { indexing, indexWorkspace } = useLounge();
  return (
    <div
      data-qa="index-empty"
      className={`flex min-h-[12rem] w-full flex-col items-center justify-center gap-3 rounded border border-dashed border-outline-variant bg-surface-container-high/40 px-4 py-8 text-center ${className}`}
    >
      <p className="font-body text-body font-semibold text-on-surface">{detail}</p>
      <p className="max-w-md font-body text-meta leading-normal text-on-surface-variant">
        Index a workspace to populate the semantic map, project health, and dead-symbol KPIs.
      </p>
      <button
        type="button"
        onClick={() => void indexWorkspace()}
        disabled={indexing}
        className="min-h-8 rounded-lg bg-primary-container px-3 py-1.5 font-body text-body font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-60"
      >
        {indexing ? "Scanning..." : "Index Workspace Now"}
      </button>
    </div>
  );
}
