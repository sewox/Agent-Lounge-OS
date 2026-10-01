"use client";

import { Suspense } from "react";
import HealthPageClient from "./health-client";

export default function HealthPage() {
  return (
    <Suspense
      fallback={
        <div className="flex h-full min-h-0 w-full items-center justify-center font-body text-body text-on-surface-variant">
          Loading health…
        </div>
      }
    >
      <HealthPageClient />
    </Suspense>
  );
}
