import { createElement, type ReactNode } from "react";
import {
  isEnableGraphUiDisabled,
  type GraphUiButtonStatus,
} from "@/lib/graph-ui-button-state";

export type GraphUiButtonViewLabels = {
  enable: string;
  enableWithPort: string;
  open: string;
  indexFirst: string;
  busyEllipsis: string;
};

export type GraphUiButtonViewProps = {
  status: GraphUiButtonStatus;
  busy: boolean;
  toast?: string | null;
  /** Auto remap info (non-error). */
  infoNote?: string | null;
  /** Translated conflict reason (User busy/foreign, or Auto band exhausted). */
  conflictNote?: string | null;
  labels: GraphUiButtonViewLabels;
  onEnable: () => void | Promise<void>;
  onOpen: () => void | Promise<void>;
};

/** Presentational Graph UI controls (createElement — unit-testable without .tsx). */
export function GraphUiButtonView({
  status,
  busy,
  toast,
  infoNote,
  conflictNote,
  labels,
  onEnable,
  onOpen,
}: GraphUiButtonViewProps): ReactNode {
  const showEnable = !status.ui_available;
  const showOpen = status.ui_available && status.project_indexed;
  const showIndexedHint = status.ui_available && !status.project_indexed;
  const enableDisabled = isEnableGraphUiDisabled(busy, status);
  const showInfo = Boolean(infoNote?.trim());
  const showConflict = Boolean(status.port_conflict && conflictNote?.trim());
  const enableLabel = showInfo ? labels.enableWithPort : labels.enable;
  const buttonTitle = showConflict
    ? conflictNote || undefined
    : showInfo
      ? infoNote || undefined
      : undefined;

  const children: ReactNode[] = [];

  if (showEnable) {
    children.push(
      createElement(
        "button",
        {
          key: "enable",
          type: "button",
          "data-qa": "graph-ui-enable",
          disabled: enableDisabled,
          title: buttonTitle,
          onClick: () => {
            void onEnable();
          },
          className:
            "rounded border border-outline-variant bg-surface-container-high px-2.5 py-1 font-body text-meta font-semibold text-on-surface hover:bg-surface-bright disabled:cursor-not-allowed disabled:opacity-50",
        },
        busy ? labels.busyEllipsis : enableLabel,
      ),
    );
  }

  if (showOpen) {
    children.push(
      createElement(
        "button",
        {
          key: "open",
          type: "button",
          "data-qa": "graph-ui-open",
          disabled: busy,
          onClick: () => {
            void onOpen();
          },
          className:
            "rounded bg-primary-container px-2.5 py-1 font-body text-meta font-semibold text-on-primary-container hover:bg-primary-dim hover:text-on-primary-fixed disabled:opacity-50",
        },
        busy ? labels.busyEllipsis : labels.open,
      ),
    );
  }

  if (showIndexedHint) {
    children.push(
      createElement(
        "button",
        {
          key: "hint",
          type: "button",
          disabled: true,
          title: labels.indexFirst,
          className:
            "cursor-not-allowed rounded border border-outline-variant/60 bg-surface-container-high/50 px-2.5 py-1 font-body text-meta text-on-surface-variant opacity-70",
        },
        labels.open,
      ),
    );
  }

  if (showConflict) {
    children.push(
      createElement(
        "p",
        {
          key: "conflict",
          className: "max-w-[18rem] text-right font-body text-meta text-error",
          role: "status",
          "data-qa": "graph-ui-conflict-note",
        },
        conflictNote,
      ),
    );
  }

  if (showInfo) {
    children.push(
      createElement(
        "p",
        {
          key: "info",
          className: "max-w-[18rem] text-right font-body text-meta text-on-surface-variant",
          role: "status",
          "data-qa": "graph-ui-auto-info",
        },
        infoNote,
      ),
    );
  }

  if (toast) {
    children.push(
      createElement(
        "p",
        {
          key: "toast",
          className: "max-w-[18rem] text-right font-body text-meta text-error",
          role: "status",
        },
        toast,
      ),
    );
  }

  return createElement(
    "div",
    {
      className: "flex flex-col items-end gap-1",
      "data-qa": "graph-ui-button",
    },
    ...children,
  );
}
