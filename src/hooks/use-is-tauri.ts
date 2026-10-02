"use client";

import { useSyncExternalStore } from "react";
import { isTauri } from "@/lib/lounge";

/**
 * Tauri host presence is fixed for a webview session; empty subscribe is enough.
 * Server + hydration assume browser (`false`); client snapshot reads
 * `window.__TAURI_INTERNALS__` via {@link isTauri}.
 */
function subscribeTauriHost(_onStoreChange: () => void): () => void {
  void _onStoreChange;
  return () => {};
}

function getServerTauriHostSnapshot(): boolean {
  return false;
}

/**
 * SSR-stable Tauri host detection. Server and hydration assume browser;
 * after hydration the client snapshot reports the real host (StrictMode-safe —
 * no module-level hydrated flag).
 */
export function useIsTauri(): boolean {
  return useSyncExternalStore(
    subscribeTauriHost,
    isTauri,
    getServerTauriHostSnapshot,
  );
}
