"use client";

import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  isDesktopDeepLinkRegistered,
  isTauri,
  registerDesktopDeepLink,
} from "@/lib/lounge";

/**
 * AppImage / Linux+Windows: user-consent runtime registration of agentlounge://.
 * Dev mode does not auto-register (documented in docs/control.md).
 */
export function DesktopIntegrationSettingsPanel() {
  const { t } = useTranslation("settings");
  const [registered, setRegistered] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [probed, setProbed] = useState(false);

  const probe = useCallback(async () => {
    if (!isTauri()) {
      setProbed(true);
      return;
    }
    try {
      const ok = await isDesktopDeepLinkRegistered();
      setRegistered(ok);
    } catch {
      setRegistered(false);
    } finally {
      setProbed(true);
    }
  }, []);

  const onEnable = useCallback(async () => {
    if (!isTauri()) {
      setError(t("desktopIntegrationBrowser"));
      return;
    }
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      if (!probed) {
        await probe();
      }
      await registerDesktopDeepLink();
      setRegistered(true);
      setMessage(t("desktopIntegrationEnabled"));
    } catch (err) {
      const text = err instanceof Error ? err.message : String(err);
      if (/unsupported|macOS|Info\.plist/i.test(text)) {
        setError(t("desktopIntegrationUnavailable"));
      } else {
        setError(t("desktopIntegrationFailed"));
      }
    } finally {
      setBusy(false);
    }
  }, [probe, probed, t]);

  return (
    <div
      data-qa="desktop-integration-settings"
      className="rounded-lg border border-outline-variant bg-surface-container"
    >
      <div className="border-b border-outline-variant bg-surface-container-low p-2.5">
        <h2 className="font-body text-panel font-semibold tracking-label text-on-surface uppercase">
          {t("desktopIntegrationTitle")}
        </h2>
        <p className="mt-1 font-body text-body leading-normal text-on-surface-variant">
          {t("desktopIntegrationDesc")}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2 p-3">
        <button
          type="button"
          data-qa="enable-desktop-integration"
          onClick={() => void onEnable()}
          onFocus={() => {
            if (!probed) void probe();
          }}
          disabled={busy || registered}
          className="rounded-lg bg-primary-container px-3 py-1.5 font-body text-body font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-60"
        >
          {t("desktopIntegrationEnable")}
        </button>
        {registered ? (
          <span
            data-qa="desktop-integration-registered"
            className="font-body text-body text-secondary"
          >
            {t("desktopIntegrationEnabled")}
          </span>
        ) : null}
        {message && !registered ? (
          <span className="font-body text-body text-on-surface-variant">{message}</span>
        ) : null}
        {error ? (
          <span className="font-body text-body text-error-dim" role="alert">
            {error}
          </span>
        ) : null}
      </div>
    </div>
  );
}
