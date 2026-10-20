"use client";

import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useTranslation } from "react-i18next";
import { VaultPanel } from "@/components/panels";
import { VaultReviewPanel } from "@/components/vault-review/vault-review-panel";

type VaultView = "map" | "review";

function parseView(raw: string | null): VaultView {
  return raw === "review" ? "review" : "map";
}

/**
 * Map view keeps the pre-review height chain (`h-full` → VaultPanel) so D960
 * stretch rows do not overflow-overlap. Review view owns the Map|Review nav
 * as its own L3 panel; map view hosts the same links inside VaultPanel chrome.
 */
export default function VaultPageClient() {
  const params = useSearchParams();
  const view = parseView(params.get("view"));
  const { t } = useTranslation("vault");

  if (view === "review") {
    return (
      <div className="flex h-full min-h-0 w-full min-w-0 flex-1 flex-col gap-2">
        <nav
          aria-label={t("review.viewsNav")}
          data-qa="panel"
          data-panel-slot="vault-view-nav"
          className="flex shrink-0 flex-wrap items-center gap-1 rounded-lg border border-outline-variant bg-surface-container-low p-1"
        >
          <VaultViewLink href="/vault" active={false} label={t("review.viewMap")} />
          <VaultViewLink
            href="/vault?view=review"
            active
            label={t("review.viewReview")}
          />
        </nav>
        <div className="flex min-h-0 min-w-0 w-full flex-1 flex-col">
          <VaultReviewPanel />
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 w-full flex-col">
      <VaultPanel />
    </div>
  );
}

function VaultViewLink({
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
      data-qa={active ? "vault-view-active" : "vault-view-link"}
      data-view={href.includes("review") ? "review" : "map"}
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
