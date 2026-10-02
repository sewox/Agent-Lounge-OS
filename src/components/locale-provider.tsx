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
import { I18nextProvider } from "react-i18next";
import { i18n, initI18n, persistLocale } from "@/lib/i18n/config";
import {
  detectInitialLocale,
  detectOsLocale,
  readStoredLocale,
  type AppLocale,
  writeStoredLocale,
} from "@/lib/i18n/locale";

type LocaleContextValue = {
  locale: AppLocale;
  setLocale: (next: AppLocale) => void;
};

const LocaleContext = createContext<LocaleContextValue | null>(null);

const listeners = new Set<() => void>();

function emitLocaleChange() {
  for (const listener of listeners) {
    listener();
  }
}

function subscribeLocale(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getLocaleSnapshot(): AppLocale {
  return detectInitialLocale(typeof window !== "undefined" ? window.localStorage : null);
}

function getServerLocaleSnapshot(): AppLocale {
  return "en";
}

// SSR-safe default — client snapshot applies OS / stored locale after hydration.
initI18n("en");

export function LocaleProvider({ children }: { children: ReactNode }) {
  const locale = useSyncExternalStore(subscribeLocale, getLocaleSnapshot, getServerLocaleSnapshot);

  useEffect(() => {
    // First launch (K5 / X-01): persist OS locale — never the SSR default ("en").
    // useSyncExternalStore hydrates with getServerSnapshot ("en"); if we wrote that
    // before the client snapshot applied, detectInitialLocale would lock EN forever.
    if (!readStoredLocale()) {
      const detected = detectOsLocale();
      writeStoredLocale(detected);
      persistLocale(detected);
      void i18n.changeLanguage(detected);
      document.documentElement.lang = detected;
      if (detected !== locale) {
        emitLocaleChange();
      }
      return;
    }
    void i18n.changeLanguage(locale);
    document.documentElement.lang = locale;
  }, [locale]);

  const setLocale = useCallback((next: AppLocale) => {
    writeStoredLocale(next);
    persistLocale(next);
    void i18n.changeLanguage(next);
    if (typeof document !== "undefined") {
      document.documentElement.lang = next;
    }
    emitLocaleChange();
  }, []);

  const value = useMemo(() => ({ locale, setLocale }), [locale, setLocale]);

  return (
    <LocaleContext.Provider value={value}>
      <I18nextProvider i18n={i18n}>{children}</I18nextProvider>
    </LocaleContext.Provider>
  );
}

export function useLocale(): LocaleContextValue {
  const ctx = useContext(LocaleContext);
  if (!ctx) {
    throw new Error("useLocale must be used within LocaleProvider");
  }
  return ctx;
}
