"use client";

import { useEffect, useSyncExternalStore } from "react";
import { isTauri } from "@/lib/lounge";

const listeners = new Set<() => void>();

/** Post-hydration Tauri flag — server + first client snapshot stay `false`. */
let clientTauriHost = false;
let hydrated = false;

function subscribeTauriHost(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getTauriHostSnapshot(): boolean {
  return clientTauriHost;
}

function getServerTauriHostSnapshot(): boolean {
  return false;
}

function emitTauriHostChange() {
  for (const listener of listeners) {
    listener();
  }
}

/**
 * SSR-stable Tauri host detection. Server and first client paint assume
 * browser; Tauri webview is detected after mount.
 */
export function useIsTauri(): boolean {
  const tauriHost = useSyncExternalStore(
    subscribeTauriHost,
    getTauriHostSnapshot,
    getServerTauriHostSnapshot,
  );

  useEffect(() => {
    if (hydrated) {
      return;
    }
    hydrated = true;
    const id = window.setTimeout(() => {
      clientTauriHost = isTauri();
      emitTauriHostChange();
    }, 0);
    return () => window.clearTimeout(id);
  }, []);

  return tauriHost;
}

/** Reset module state for unit tests. */
export function resetTauriHostStoreForTests(): void {
  clientTauriHost = false;
  hydrated = false;
  listeners.clear();
}
