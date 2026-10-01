"use client";

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApprovalAlertEngine } from "@/lib/approval-alert/engine";
import {
  BUILTIN_SOUND_IDS,
  readApprovalSoundSettings,
  sanitizeCustomSoundFileName,
  writeApprovalSoundSettings,
  type ApprovalSoundSettings,
  type BuiltinSoundId,
} from "@/lib/approval-alert/settings";
import { isTauri } from "@/lib/lounge";

/** Minimal sound prefs surface for PR-2b (full Settings UI lands in PR-5). */
export function ApprovalSoundSettingsPanel() {
  const { t } = useTranslation("settings");
  const [settings, setSettings] = useState<ApprovalSoundSettings>(() => readApprovalSoundSettings());
  const [customLabel, setCustomLabel] = useState<string | null>(
    sanitizeCustomSoundFileName(settings.customFileName),
  );
  const [error, setError] = useState<string | null>(null);

  const persist = useCallback((next: ApprovalSoundSettings) => {
    const safe: ApprovalSoundSettings = {
      ...next,
      customFileName: sanitizeCustomSoundFileName(next.customFileName),
    };
    setSettings(safe);
    writeApprovalSoundSettings(safe);
    setCustomLabel(safe.customFileName);
  }, []);

  const pickCustom = async () => {
    if (!isTauri()) {
      setError("Custom sounds require the desktop app");
      return;
    }
    setError(null);
    try {
      const fileName = await invoke<string>("pick_custom_approval_sound");
      const bare = sanitizeCustomSoundFileName(fileName);
      if (!bare) {
        setError("invalid custom sound file name");
        return;
      }
      // Verify the file is loadable before persisting the bare name.
      await invoke<string>("load_custom_approval_sound_data_url", { fileName: bare });
      persist({
        ...settings,
        soundId: "custom",
        customFileName: bare,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (message !== "cancelled") {
        setError(message);
      }
    }
  };

  const preview = () => {
    const engine = new ApprovalAlertEngine({
      readSettings: () => settings,
      loadCustomSound: async (fileName) =>
        invoke<string>("load_custom_approval_sound_data_url", { fileName }),
    });
    engine.preview();
  };

  return (
    <div
      data-qa="approval-sound"
      className="rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("alertSound")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("alertSoundDesc")}
        </p>
      </div>
      <div className="space-y-3 p-3">
        <label className="flex items-center justify-between gap-3 font-body text-body">
          <span>{t("soundEnabled")}</span>
          <input
            type="checkbox"
            role="switch"
            checked={settings.enabled}
            onChange={(event) => persist({ ...settings, enabled: event.target.checked })}
            className="accent-primary"
          />
        </label>
        <label className="block space-y-1 font-body text-meta text-on-surface-variant">
          {t("soundPreset")}
          <select
            value={settings.soundId === "custom" ? "chime-soft" : settings.soundId}
            onChange={(event) =>
              persist({
                ...settings,
                soundId: event.target.value as BuiltinSoundId,
                customFileName: null,
              })
            }
            className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-body text-body text-on-surface"
          >
            {BUILTIN_SOUND_IDS.map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
        </label>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void pickCustom()}
            className="rounded border border-outline-variant bg-surface-container-high px-3 py-1.5 font-body text-body hover:bg-surface-bright"
          >
            {t("pickCustomSound")}
          </button>
          {customLabel ? (
            <span className="font-mono text-meta text-on-surface-variant">{customLabel}</span>
          ) : null}
        </div>
        <p className="font-body text-meta text-outline">{t("customSound")}</p>
        <label className="block space-y-1 font-body text-meta text-on-surface-variant">
          {t("volume")} ({Math.round(settings.volume * 100)}%)
          <input
            type="range"
            min={0}
            max={100}
            value={Math.round(settings.volume * 100)}
            onChange={(event) =>
              persist({ ...settings, volume: Number(event.target.value) / 100 })
            }
            className="w-full accent-primary"
          />
        </label>
        <label className="block space-y-1 font-body text-meta text-on-surface-variant">
          {t("interval")} ({settings.intervalSecs}s)
          <input
            type="number"
            min={5}
            max={600}
            value={settings.intervalSecs}
            onChange={(event) =>
              persist({ ...settings, intervalSecs: Number(event.target.value) || 60 })
            }
            className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-body text-body text-on-surface"
          />
        </label>
        <button
          type="button"
          onClick={preview}
          className="rounded border border-primary/40 bg-primary-container/20 px-3 py-1.5 font-body text-body font-semibold text-primary hover:bg-primary-container/35"
        >
          {t("testListen")}
        </button>
        {error ? <p className="font-body text-meta text-error">{error}</p> : null}
      </div>
    </div>
  );
}
