import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import enApprovals from "./locales/en/approvals.json";
import enCommon from "./locales/en/common.json";
import enDashboard from "./locales/en/dashboard.json";
import enSettings from "./locales/en/settings.json";
import enShell from "./locales/en/shell.json";
import trApprovals from "./locales/tr/approvals.json";
import trCommon from "./locales/tr/common.json";
import trDashboard from "./locales/tr/dashboard.json";
import trSettings from "./locales/tr/settings.json";
import trShell from "./locales/tr/shell.json";
import { detectInitialLocale, LOCALE_KEY, type AppLocale } from "./locale";

/**
 * Only real dictionaries — no aliased namespaces that hide untranslated surfaces.
 * vault / health / fleet / quotas / telemetry / onboarding / palette leftovers
 * are still hardcoded; listed honestly in the PR body.
 */
export const I18N_NAMESPACES = [
  "shell",
  "dashboard",
  "settings",
  "approvals",
  "common",
] as const;

export type I18nNamespace = (typeof I18N_NAMESPACES)[number];

const resources = {
  en: {
    shell: enShell,
    dashboard: enDashboard,
    settings: enSettings,
    approvals: enApprovals,
    common: enCommon,
  },
  tr: {
    shell: trShell,
    dashboard: trDashboard,
    settings: trSettings,
    approvals: trApprovals,
    common: trCommon,
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
