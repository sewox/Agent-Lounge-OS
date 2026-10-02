import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import enApprovals from "./locales/en/approvals.json";
import enCommon from "./locales/en/common.json";
import enDashboard from "./locales/en/dashboard.json";
import enHealth from "./locales/en/health.json";
import enOnboarding from "./locales/en/onboarding.json";
import enSettings from "./locales/en/settings.json";
import enShell from "./locales/en/shell.json";
import enVault from "./locales/en/vault.json";
import trApprovals from "./locales/tr/approvals.json";
import trCommon from "./locales/tr/common.json";
import trDashboard from "./locales/tr/dashboard.json";
import trHealth from "./locales/tr/health.json";
import trOnboarding from "./locales/tr/onboarding.json";
import trSettings from "./locales/tr/settings.json";
import trShell from "./locales/tr/shell.json";
import trVault from "./locales/tr/vault.json";
import { detectInitialLocale, LOCALE_KEY, type AppLocale } from "./locale";

/**
 * Real dictionaries for primary namespaces.
 * fleet / quotas / telemetry may still have residual mixed copy.
 */
export const I18N_NAMESPACES = [
  "shell",
  "dashboard",
  "settings",
  "approvals",
  "common",
  "vault",
  "health",
  "onboarding",
] as const;

export type I18nNamespace = (typeof I18N_NAMESPACES)[number];

const resources = {
  en: {
    shell: enShell,
    dashboard: enDashboard,
    settings: enSettings,
    approvals: enApprovals,
    common: enCommon,
    vault: enVault,
    health: enHealth,
    onboarding: enOnboarding,
  },
  tr: {
    shell: trShell,
    dashboard: trDashboard,
    settings: trSettings,
    approvals: trApprovals,
    common: trCommon,
    vault: trVault,
    health: trHealth,
    onboarding: trOnboarding,
  },
} as const;

let initialized = false;

export function initI18n(locale: AppLocale = detectInitialLocale()): typeof i18n {
  if (initialized) {
    if (i18n.language !== locale) {
      void i18n.changeLanguage(locale);
    }
    return i18n;
  }

  void i18n.use(initReactI18next).init({
    resources,
    lng: locale,
    fallbackLng: "en",
    supportedLngs: ["en", "tr"],
    ns: [...I18N_NAMESPACES],
    defaultNS: "common",
    interpolation: { escapeValue: false },
    react: { useSuspense: false },
  });

  initialized = true;
  return i18n;
}

export function persistLocale(locale: AppLocale): void {
  try {
    localStorage.setItem(LOCALE_KEY, locale);
  } catch {
    /* ignore */
  }
}

export { i18n, LOCALE_KEY };
