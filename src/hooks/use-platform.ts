"use client";

import { useEffect, useSyncExternalStore } from "react";
import {
  detectPlatform,
  SSR_DEFAULT_PLATFORM,
  type LoungePlatform,
} from "@/lib/platform";

const listeners = new Set<() => void>();
let clientPlatform: LoungePlatform = SSR_DEFAULT_PLATFORM;
let hydrated = false;

function subscribePlatform(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getPlatformSnapshot(): LoungePlatform {
  return clientPlatform;
}

function getServerPlatformSnapshot(): LoungePlatform {
  return SSR_DEFAULT_PLATFORM;
}

function emitPlatformChange() {
  for (const listener of listeners) {
    listener();
  }
}

/**
 * SSR-stable platform detection. Server and first client paint use
 * {@link SSR_DEFAULT_PLATFORM}; real OS is applied after mount.
 */
export function usePlatform(): LoungePlatform {
  const platform = useSyncExternalStore(
    subscribePlatform,
    getPlatformSnapshot,
    getServerPlatformSnapshot,
  );

  useEffect(() => {
    if (hydrated) {
      return;
    }
    hydrated = true;
    const id = window.setTimeout(() => {
      clientPlatform = detectPlatform();
      emitPlatformChange();
    }, 0);
    return () => window.clearTimeout(id);
  }, []);

  return platform;
}

/** Reset module state for unit tests. */
export function resetPlatformStoreForTests(): void {
  clientPlatform = SSR_DEFAULT_PLATFORM;
  hydrated = false;
  listeners.clear();
}
