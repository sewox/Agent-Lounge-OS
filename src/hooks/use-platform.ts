"use client";

import { useSyncExternalStore } from "react";
import {
  detectPlatform,
  SSR_DEFAULT_PLATFORM,
  type LoungePlatform,
} from "@/lib/platform";

/**
 * Platform does not change during a session; an empty subscribe is enough.
 * Server + hydration use {@link getServerPlatformSnapshot}; the client
 * snapshot reads the real OS via {@link detectPlatform}.
 */
function subscribePlatform(_onStoreChange: () => void): () => void {
  void _onStoreChange;
  return () => {};
}

function getServerPlatformSnapshot(): LoungePlatform {
  return SSR_DEFAULT_PLATFORM;
}

/**
 * SSR-stable platform detection. Server and hydration stay on
 * {@link SSR_DEFAULT_PLATFORM}; after hydration the client snapshot
 * reports the real OS (StrictMode-safe — no module-level hydrated flag).
 */
export function usePlatform(): LoungePlatform {
  return useSyncExternalStore(
    subscribePlatform,
    detectPlatform,
    getServerPlatformSnapshot,
  );
}
