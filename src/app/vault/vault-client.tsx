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

export default function VaultPageClient() {
  const params = useSearchParams();
  const view = parseView(params.get("view"));
  const { t } = useTranslation("vault");

  return (
    <div className="flex h-full min-h-0 w-full flex-col gap-2">
      <nav
        aria-label={t("review.viewsNav")}
        data-qa="vault-view-nav"
        className="flex shrink-0 flex-wrap items-center gap-1 rounded-lg border border-outline-variant bg-surface-container-low p-1"
      >
        <VaultViewLink href="/vault" active={view === "map"} label={t("review.viewMap")} />
        <VaultViewLink
          href="/vault?view=review"
          active={view === "review"}
          label={t("review.viewReview")}
        />
      </nav>
      <div className="min-h-0 flex-1">
        {view === "review" ? (
          <VaultReviewPanel />
        ) : (
          <div className="flex h-full min-h-0 w-full flex-col">
            <VaultPanel />
          </div>
        )}
      </div>
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
