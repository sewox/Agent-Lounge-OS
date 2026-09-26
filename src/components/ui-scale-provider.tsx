"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import {
  DEFAULT_UI_SCALE,
  nextUiScale,
  readStoredUiScale,
  type UiScale,
  UI_SCALES,
  UI_SCALE_KEY,
  writeStoredUiScale,
} from "@/lib/ui-prefs";

type UiScaleContextValue = {
  scale: UiScale;
  setScale: (next: UiScale) => void;
  bump: (direction: 1 | -1) => void;
  reset: () => void;
  scales: readonly UiScale[];
};

const UiScaleContext = createContext<UiScaleContextValue | null>(null);

const listeners = new Set<() => void>();

function emitScaleChange() {
  for (const listener of listeners) {
    listener();
  }
}

function subscribeScale(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getScaleSnapshot(): UiScale {
  return readStoredUiScale(typeof window !== "undefined" ? window.localStorage : null);
}

function getServerScaleSnapshot(): UiScale {
  return DEFAULT_UI_SCALE;
}

function applyRootScale(scale: UiScale) {
  if (typeof document === "undefined") {
    return;
  }
  const root = document.documentElement;
  root.style.setProperty("--ui-scale", String(scale));
  root.style.fontSize = `${16 * scale}px`;
  root.dataset.uiScale = String(scale);
}

export function UiScaleProvider({ children }: { children: ReactNode }) {
  const scale = useSyncExternalStore(subscribeScale, getScaleSnapshot, getServerScaleSnapshot);

  useEffect(() => {
    applyRootScale(scale);
  }, [scale]);

  const setScale = useCallback((next: UiScale) => {
    writeStoredUiScale(next, typeof window !== "undefined" ? window.localStorage : null);
    applyRootScale(next);
    emitScaleChange();
  }, []);

  const bump = useCallback(
    (direction: 1 | -1) => {
      setScale(nextUiScale(scale, direction));
    },
    [scale, setScale],
  );

  const reset = useCallback(() => {
    setScale(DEFAULT_UI_SCALE);
  }, [setScale]);

  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === UI_SCALE_KEY || event.key === null) {
        emitScaleChange();
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey) {
        return;
      }
      const key = event.key;
      if (key === "=" || key === "+") {
        event.preventDefault();
        setScale(nextUiScale(scale, 1));
        return;
      }
      if (key === "-" || key === "_") {
        event.preventDefault();
        setScale(nextUiScale(scale, -1));
        return;
      }
      if (key === "0") {
        event.preventDefault();
        setScale(DEFAULT_UI_SCALE);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [scale, setScale]);

  const value = useMemo(
    () => ({ scale, setScale, bump, reset, scales: UI_SCALES }),
    [scale, setScale, bump, reset],
  );

  return <UiScaleContext.Provider value={value}>{children}</UiScaleContext.Provider>;
}

export function useUiScale(): UiScaleContextValue {
  const ctx = useContext(UiScaleContext);
  if (!ctx) {
    throw new Error("useUiScale must be used within UiScaleProvider");
  }
  return ctx;
}
