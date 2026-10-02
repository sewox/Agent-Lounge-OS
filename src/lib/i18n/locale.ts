export const LOCALE_KEY = "lounge.locale";

export type AppLocale = "en" | "tr";

export const APP_LOCALES: AppLocale[] = ["en", "tr"];

export function isAppLocale(value: unknown): value is AppLocale {
  return value === "en" || value === "tr";
}

export function normalizeLocaleTag(raw: string | null | undefined): AppLocale | null {
  if (!raw) {
    return null;
  }
  const tag = raw.trim().toLowerCase();
  if (tag.startsWith("tr")) {
    return "tr";
  }
  if (tag.startsWith("en")) {
    return "en";
  }
  return null;
}

export function detectOsLocale(
  nav?: Pick<Navigator, "language" | "languages"> | null,
): AppLocale {
  const source =
    nav === undefined ? (typeof navigator !== "undefined" ? navigator : null) : nav;
  if (!source) {
    return "en";
  }
  const candidates = [source.language, ...(source.languages ?? [])];
  for (const tag of candidates) {
    const locale = normalizeLocaleTag(tag);
    if (locale) {
      return locale;
    }
  }
  return "en";
}

export function readStoredLocale(storage?: Pick<Storage, "getItem"> | null): AppLocale | null {
  const store =
    storage === undefined && typeof localStorage !== "undefined" ? localStorage : storage;
  try {
    const raw = store?.getItem(LOCALE_KEY) ?? store?.getItem("locale");
    return normalizeLocaleTag(raw);
  } catch {
    return null;
  }
}

export function detectInitialLocale(
  storage?: Pick<Storage, "getItem"> | null,
  nav?: Pick<Navigator, "language" | "languages"> | null,
): AppLocale {
  return readStoredLocale(storage) ?? detectOsLocale(nav);
}

export function writeStoredLocale(
  locale: AppLocale,
  storage?: Pick<Storage, "setItem"> | null,
): void {
  const store =
    storage === undefined && typeof localStorage !== "undefined" ? localStorage : storage;
  try {
    store?.setItem(LOCALE_KEY, locale);
  } catch {
    /* ignore */
  }
}
