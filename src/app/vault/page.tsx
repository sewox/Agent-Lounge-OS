"use client";

import { Suspense } from "react";
import VaultPageClient from "./vault-client";

export default function VaultPage() {
  return (
    <Suspense
      fallback={
        <div className="flex h-full min-h-0 w-full items-center justify-center font-body text-body text-on-surface-variant">
          Loading vault…
        </div>
      }
    >
      <VaultPageClient />
    </Suspense>
  );
}
