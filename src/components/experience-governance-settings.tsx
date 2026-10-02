"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@/lib/lounge";

const MIN_TTL = 1;
const MAX_TTL = 3650;

export function ExperienceGovernanceSettingsPanel() {
  const { t } = useTranslation("settings");
  const [ttlDays, setTtlDays] = useState(90);
  const [useCountThreshold, setUseCountThreshold] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    const load = async () => {
      setLoading(true);
      try {
        const [ttl, threshold] = await Promise.all([
          invoke<number>("get_experience_ttl_days"),
          invoke<number>("get_experience_use_count_threshold"),
        ]);
        if (!cancelled) {
          setTtlDays(ttl);
          setUseCountThreshold(threshold);
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, []);

  const saveTtl = async (value: number) => {
    const clamped = Math.min(MAX_TTL, Math.max(MIN_TTL, Math.round(value)));
    setTtlDays(clamped);
    if (!isTauri()) {
      return;
    }
    setError(null);
    try {
      const saved = await invoke<number>("set_experience_ttl_days", { days: clamped });
      setTtlDays(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const saveUseCount = async (value: number) => {
    const clamped = Math.max(0, Math.round(value));
    setUseCountThreshold(clamped);
    if (!isTauri()) {
      return;
    }
    setError(null);
    try {
      const saved = await invoke<number>("set_experience_use_count_threshold", {
        threshold: clamped,
      });
      setUseCountThreshold(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div
      data-qa="experience-governance"
      className="rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("autoArchiveTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("autoArchiveDesc")}
        </p>
      </div>
      <div className="space-y-3 p-3">
        {loading ? (
          <p className="font-body text-meta text-outline">{t("loading")}</p>
        ) : (
          <>
            <label className="block space-y-1 font-body text-meta text-on-surface-variant">
              {t("ttlDays")} ({ttlDays})
              <input
                type="range"
                min={MIN_TTL}
                max={365}
                value={Math.min(ttlDays, 365)}
                onChange={(event) => void saveTtl(Number(event.target.value))}
                className="w-full accent-primary"
                data-qa="ttl-days"
              />
            </label>
            <label className="block space-y-1 font-body text-meta text-on-surface-variant">
              {t("ttlDaysExact")}
              <input
                type="number"
                min={MIN_TTL}
                max={MAX_TTL}
                value={ttlDays}
                onChange={(event) => setTtlDays(Number(event.target.value) || MIN_TTL)}
                onBlur={() => void saveTtl(ttlDays)}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-body text-body text-on-surface"
              />
            </label>
            <label className="block space-y-1 font-body text-meta text-on-surface-variant">
              {t("useCountThreshold")}
              <input
                type="number"
                min={0}
                max={1_000_000}
                value={useCountThreshold}
                onChange={(event) => setUseCountThreshold(Number(event.target.value) || 0)}
                onBlur={() => void saveUseCount(useCountThreshold)}
                className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-body text-body text-on-surface"
                data-qa="use-count-threshold"
              />
            </label>
            <p className="font-body text-meta text-outline">{t("useCountThresholdHint")}</p>
          </>
        )}
        {error ? <p className="font-body text-meta text-error">{error}</p> : null}
      </div>
    </div>
  );
}
