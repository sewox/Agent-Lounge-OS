"use client";

import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { useAlertdialogFocusTrap } from "@/hooks/use-alertdialog-focus-trap";
import type { ConnectPlan } from "@/lib/lounge";

type Mode = "plan" | "jsonc" | "print";

export type AgentConnectDialogProps = {
  open: boolean;
  mode: Mode;
  repair: boolean;
  client: string;
  plan: ConnectPlan | null;
  printPayload: unknown;
  busy: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirmApply: () => void;
  onWriteAnyway: () => void;
  onPrintOnly: () => void;
};

export function AgentConnectDialog({
  open,
  mode,
  repair,
  client,
  plan,
  printPayload,
  busy,
  error,
  onCancel,
  onConfirmApply,
  onWriteAnyway,
  onPrintOnly,
}: AgentConnectDialogProps) {
  const { t } = useTranslation("health");
  const containerRef = useRef<HTMLDivElement | null>(null);

  useAlertdialogFocusTrap({
    open,
    containerRef,
    instanceKey: `${mode}:${client}`,
    initialFocusSelector:
      mode === "jsonc"
        ? '[data-qa="connect-jsonc-print"]'
        : '[data-qa="connect-confirm"]',
    escapeEnabled: !busy,
    onEscape: onCancel,
  });

  if (!open) return null;

  const title =
    mode === "jsonc"
      ? t("agents.jsoncConsentTitle")
      : mode === "print"
        ? t("agents.printResultTitle")
        : repair
          ? t("agents.repairPlanTitle")
          : t("agents.connectPlanTitle");

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-scrim/50 p-4"
      data-qa="connect-dialog-backdrop"
    >
      <div
        ref={containerRef}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="connect-dialog-title"
        data-qa="connect-dialog"
        data-mode={mode}
        className="max-h-[90vh] w-full max-w-xl overflow-auto rounded-lg border border-outline-variant bg-surface-container p-4 shadow-lg"
      >
        <h2
          id="connect-dialog-title"
          className="font-body text-panel font-semibold tracking-label text-on-surface uppercase"
        >
          {title}
        </h2>
        <p className="mt-1 font-mono text-meta text-outline">{client}</p>

        {mode === "jsonc" ? (
          <>
            <p className="mt-3 font-body text-body text-on-surface">
              {t("agents.jsoncConsentLead")}
            </p>
            <p className="mt-2 font-body text-body text-on-surface-variant">
              {t("agents.jsoncConsentPrompt", {
                write: t("agents.jsoncWriteAnyway"),
                print: t("agents.jsoncPrintOnly"),
              })}
            </p>
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                type="button"
                data-qa="connect-jsonc-write"
                disabled={busy}
                onClick={onWriteAnyway}
                className="min-h-8 rounded border border-primary/40 bg-primary-container px-3 py-1 font-body text-body font-medium text-on-primary-container focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:opacity-60"
              >
                {t("agents.jsoncWriteAnyway")}
              </button>
              <button
                type="button"
                data-qa="connect-jsonc-print"
                disabled={busy}
                onClick={onPrintOnly}
                className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body text-on-surface focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:opacity-60"
              >
                {t("agents.jsoncPrintOnly")}
              </button>
              <button
                type="button"
                data-qa="connect-cancel"
                disabled={busy}
                onClick={onCancel}
                className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body text-on-surface-variant"
              >
                {t("agents.connectCancel")}
              </button>
            </div>
          </>
        ) : null}

        {mode === "plan" && plan ? (
          <>
            <p className="mt-3 font-body text-body text-on-surface-variant">
              {t("agents.connectPlanLead")}
            </p>
            <pre
              data-qa="connect-plan-diff"
              tabIndex={0}
              className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded border border-outline-variant/40 bg-surface-container-low p-2 font-mono text-meta text-on-surface focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
            >
              {plan.files
                .map((f) => {
                  const before = f.entry_before
                    ? JSON.stringify(f.entry_before, null, 2)
                    : "null";
                  const after = f.entry_after
                    ? JSON.stringify(f.entry_after, null, 2)
                    : "null";
                  return `${f.path_masked}\n${f.diff}\n--- entry_before ---\n${before}\n--- entry_after ---\n${after}`;
                })
                .join("\n\n")}
            </pre>
            {plan.warnings.length > 0 ? (
              <ul className="mt-2 list-disc pl-5 font-body text-meta text-on-surface-variant">
                {plan.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            ) : null}
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                type="button"
                data-qa="connect-confirm"
                disabled={busy}
                onClick={onConfirmApply}
                className="min-h-8 rounded border border-primary/40 bg-primary-container px-3 py-1 font-body text-body font-medium text-on-primary-container focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:opacity-60"
              >
                {busy ? t("agents.connectBusy") : t("agents.connectConfirm")}
              </button>
              <button
                type="button"
                data-qa="connect-cancel"
                disabled={busy}
                onClick={onCancel}
                className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body text-on-surface-variant"
              >
                {t("agents.connectCancel")}
              </button>
            </div>
          </>
        ) : null}

        {mode === "print" ? (
          <>
            <pre
              data-qa="connect-print-payload"
              tabIndex={0}
              className="mt-3 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded border border-outline-variant/40 bg-surface-container-low p-2 font-mono text-meta text-on-surface focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
            >
              {JSON.stringify(printPayload, null, 2)}
            </pre>
            <div className="mt-4">
              <button
                type="button"
                data-qa="connect-cancel"
                onClick={onCancel}
                className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body text-on-surface"
              >
                {t("agents.connectCancel")}
              </button>
            </div>
          </>
        ) : null}

        {error ? (
          <p data-qa="connect-error" className="mt-3 font-body text-body text-error">
            {t("agents.connectError", { message: error })}
          </p>
        ) : null}
      </div>
    </div>
  );
}

/** Which primary action the Health agents row offers. */
export function connectActionForStatus(
  status: string,
): "connect" | "repair" | null {
  switch (status) {
    case "missing":
    case "not_installed":
      return "connect";
    case "stale":
    case "non_canonical":
    case "hash_mismatch":
      return "repair";
    default:
      return null;
  }
}
