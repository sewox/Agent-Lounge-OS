"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@/lib/lounge";

type EditorPreset = "default" | "vscode" | "cursor" | "custom";

type EditorSettings = {
  preset: EditorPreset;
  custom_program: string;
  custom_args_template: string;
};

const PRESETS: EditorPreset[] = ["default", "vscode", "cursor", "custom"];

export function EditorSettingsPanel() {
  const { t } = useTranslation("settings");
  const [settings, setSettings] = useState<EditorSettings>({
    preset: "default",
    custom_program: "",
    custom_args_template: "{path}",
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [testMessage, setTestMessage] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    let cancelled = false;
    const load = async () => {
      setLoading(true);
      setError(null);
      try {
        const row = await invoke<EditorSettings>("get_editor_settings");
        if (!cancelled) {
          setSettings(row);
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

  const persist = async (next: EditorSettings) => {
    setSettings(next);
    if (!isTauri()) {
      return;
    }
    setError(null);
    try {
      const saved = await invoke<EditorSettings>("set_editor_settings", { settings: next });
      setSettings(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const testEditor = async () => {
    if (!isTauri()) {
      setTestMessage(t("editorDesktopOnly"));
      return;
    }
    setTestMessage(null);
    setError(null);
    try {
      await invoke("test_editor_settings", { settings });
      setTestMessage(t("editorTestOk"));
    } catch (err) {
      setTestMessage(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div
      data-qa="editor-settings"
      className="rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("editorTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("editorDesc")}
        </p>
      </div>
      <div className="space-y-3 p-3">
        {loading ? (
          <p className="font-body text-meta text-outline">{t("loading")}</p>
        ) : (
          <>
            <fieldset className="space-y-2">
              <legend className="font-body text-meta text-on-surface-variant">{t("editorPreset")}</legend>
              <div className="flex flex-wrap gap-2" role="radiogroup" aria-label={t("editorPreset")}>
                {PRESETS.map((preset) => {
                  const selected = settings.preset === preset;
                  return (
                    <button
                      key={preset}
                      type="button"
                      role="radio"
                      aria-checked={selected}
                      onClick={() => void persist({ ...settings, preset })}
                      className={`min-h-8 rounded border px-3 py-1.5 font-body text-body ${
                        selected
                          ? "border-primary bg-primary-container/25 font-semibold text-primary"
                          : "border-outline-variant bg-surface-container-high text-on-surface hover:bg-surface-bright"
                      }`}
                    >
                      {t(`editorPreset_${preset}`)}
                    </button>
                  );
                })}
              </div>
            </fieldset>
            {settings.preset === "custom" ? (
              <div className="grid gap-2 sm:grid-cols-2">
                <label className="space-y-1 font-body text-meta text-on-surface-variant">
                  {t("editorCustomProgram")}
                  <input
                    value={settings.custom_program}
                    onChange={(event) =>
                      setSettings((current) => ({
                        ...current,
                        custom_program: event.target.value,
                      }))
                    }
                    onBlur={() => void persist(settings)}
                    className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-mono text-body text-on-surface"
                    placeholder="code"
                  />
                </label>
                <label className="space-y-1 font-body text-meta text-on-surface-variant">
                  {t("editorCustomArgs")}
                  <input
                    value={settings.custom_args_template}
                    onChange={(event) =>
                      setSettings((current) => ({
                        ...current,
                        custom_args_template: event.target.value,
                      }))
                    }
                    onBlur={() => void persist(settings)}
                    className="w-full rounded border border-outline-variant bg-surface-container-low px-2 py-1.5 font-mono text-body text-on-surface"
                    placeholder="-g {path}"
                  />
                </label>
              </div>
            ) : null}
            <button
              type="button"
              onClick={() => void testEditor()}
              className="rounded border border-primary/40 bg-primary-container/20 px-3 py-1.5 font-body text-body font-semibold text-primary hover:bg-primary-container/35"
            >
              {t("editorTest")}
            </button>
            {testMessage ? (
              <p
                className={`font-body text-meta ${testMessage === t("editorTestOk") ? "text-primary" : "text-error"}`}
              >
                {testMessage}
              </p>
            ) : null}
          </>
        )}
        {error ? <p className="font-body text-meta text-error">{error}</p> : null}
      </div>
    </div>
  );
}
