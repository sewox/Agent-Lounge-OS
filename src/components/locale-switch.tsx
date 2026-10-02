"use client";

import { useTranslation } from "react-i18next";
import { useLocale } from "@/components/locale-provider";
import type { AppLocale } from "@/lib/i18n/locale";

type LocaleSwitchProps = {
  compact?: boolean;
  className?: string;
};

const OPTIONS: { id: AppLocale; labelKey: "english" | "turkish"; shortKey: "localeEn" | "localeTr" }[] =
  [
    { id: "en", labelKey: "english", shortKey: "localeEn" },
    { id: "tr", labelKey: "turkish", shortKey: "localeTr" },
  ];

export function LocaleSwitch({ compact = false, className = "" }: LocaleSwitchProps) {
  const { locale, setLocale } = useLocale();
  const { t } = useTranslation("shell");

  return (
    <div
      data-qa="locale-switch"
      role="radiogroup"
      aria-label={t("languageLabel")}
      className={`flex flex-wrap gap-1.5 ${className}`.trim()}
    >
      {OPTIONS.map((option) => {
        const selected = locale === option.id;
        return (
          <button
            key={option.id}
            type="button"
            role="radio"
            aria-checked={selected}
            onClick={() => setLocale(option.id)}
            className={`min-h-8 rounded border px-2.5 py-1 font-body text-body ${
              selected
                ? "border-primary bg-primary-container/25 font-semibold text-primary"
                : "border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright"
            }`}
          >
            {compact ? t(option.shortKey) : t(option.labelKey)}
          </button>
        );
      })}
    </div>
  );
}
