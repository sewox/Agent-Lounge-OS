"use client";

import { Suspense } from "react";
import { useTranslation } from "react-i18next";
import VaultPageClient from "./vault-client";

function VaultLoadingFallback() {
  const { t } = useTranslation("vault");
  return (
    <div className="flex h-full min-h-0 w-full items-center justify-center font-body text-body text-on-surface-variant">
      {t("loadingVault")}
    </div>
  );
}

export default function VaultPage() {
  return (
    <Suspense fallback={<VaultLoadingFallback />}>
      <VaultPageClient />
    </Suspense>
  );
}
