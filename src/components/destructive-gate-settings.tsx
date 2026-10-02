"use client";

import { useTranslation } from "react-i18next";

const PATTERNS = [
  "rm -rf",
  "git reset --hard",
  "git push --force / -f / --force-with-lease",
  "git clean -fd / -fdx",
  "del /s",
  "rd /s",
  "Remove-Item -Recurse",
  "format",
  "DROP TABLE",
  "TRUNCATE TABLE",
  "DELETE FROM … (no WHERE)",
];

/** Static PolicyGate copy for AP-06 (Settings surface). */
export function DestructiveGateSettingsPanel() {
  const { t } = useTranslation("settings");

  return (
    <div
      data-qa="destructive-gate"
      className="rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("destructiveTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("destructiveDesc")}
        </p>
      </div>
      <div className="space-y-3 p-3">
        <p className="font-body text-body text-on-surface">{t("destructiveNeverAsk")}</p>
        <ul className="list-inside list-disc space-y-1 font-mono text-meta text-on-surface-variant">
          {PATTERNS.map((pattern) => (
            <li key={pattern}>{pattern}</li>
          ))}
        </ul>
      </div>
    </div>
  );
}
