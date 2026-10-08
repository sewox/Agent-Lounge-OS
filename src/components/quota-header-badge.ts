"use client";

import {
  createElement,
  Fragment,
  type AnchorHTMLAttributes,
  type ComponentType,
  type FocusEventHandler,
  type MouseEventHandler,
  type ReactNode,
  useEffect,
  useMemo,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import type { ToolQuota } from "@/lib/lounge";
import {
  amberRowsSignature,
  clampBadgeIndex,
  quotaBadgeName,
  quotaBadgePercent,
  quotaBadgeTone,
  QUOTA_BADGE_ROTATE_MS,
  selectAmberQuotaRows,
  type QuotaBadgeTone,
} from "@/lib/quota-badge";

export type QuotaBadgeLinkProps = AnchorHTMLAttributes<HTMLAnchorElement> & {
  href: string;
  children?: ReactNode;
  "data-testid"?: string;
  "data-amber-count"?: string;
  "data-badge-index"?: string;
  "data-paused"?: string;
};

function readPrefersReducedMotion(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
    return false;
  }
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(readPrefersReducedMotion);

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  return reduced;
}

function toneClass(tone: QuotaBadgeTone): string {
  if (tone === "error") {
    return "border-error-container bg-error-container/20 text-error-dim";
  }
  if (tone === "amber") {
    return "border-error-container/70 bg-error-container/10 text-error-dim";
  }
  return "border-outline-variant bg-surface-container-high text-on-surface-variant";
}

function toneDotClass(tone: QuotaBadgeTone): string {
  if (tone === "error" || tone === "amber") {
    return "animate-pulse bg-error";
  }
  return "bg-secondary";
}

function formatListItem(
  row: ToolQuota,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  return t("quotaBadgeListItem", {
    tool: quotaBadgeName(row),
    percent: quotaBadgePercent(row),
  });
}

function buildTooltip(
  rows: ToolQuota[],
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  if (rows.length === 0) {
    return t("quotaOk");
  }
  const lines = rows.map((row) => {
    const reset = row.reset?.trim();
    const hasReset = Boolean(reset && reset !== "—" && reset !== "-");
    if (hasReset) {
      return t("quotaBadgeTooltipLine", {
        tool: quotaBadgeName(row),
        percent: quotaBadgePercent(row),
        reset,
      });
    }
    return t("quotaBadgeTooltipLineNoReset", {
      tool: quotaBadgeName(row),
      percent: quotaBadgePercent(row),
    });
  });
  return `${t("quotaBadgeTooltipHeader")}\n${lines.join("\n")}`;
}

export type QuotaHeaderBadgeProps = {
  quotas: ToolQuota[];
  /** Navigation element (next/link in app, plain <a> in unit tests). */
  LinkComponent: ComponentType<QuotaBadgeLinkProps>;
};

export function QuotaHeaderBadge({ quotas, LinkComponent }: QuotaHeaderBadgeProps) {
  const { t } = useTranslation("shell");
  const reducedMotion = usePrefersReducedMotion();
  const amberRows = useMemo(() => selectAmberQuotaRows(quotas), [quotas]);
  const signature = useMemo(() => amberRowsSignature(amberRows), [amberRows]);
  const [rotation, setRotation] = useState({ signature, index: 0 });
  const [paused, setPaused] = useState(false);

  // Reset rotation index when the amber row set changes (React "adjust state during render").
  if (rotation.signature !== signature) {
    setRotation({ signature, index: 0 });
  }

  const index = rotation.signature === signature ? rotation.index : 0;
  const safeIndex = clampBadgeIndex(index, amberRows.length);
  const active = amberRows[safeIndex] ?? null;
  const rotate = amberRows.length > 1 && !reducedMotion && !paused;

  useEffect(() => {
    if (!rotate) {
      return;
    }
    const id = window.setInterval(() => {
      setRotation((prev) => {
        const len = amberRows.length;
        if (len <= 0) {
          return { signature, index: 0 };
        }
        const current = prev.signature === signature ? prev.index : 0;
        return {
          signature,
          index: (clampBadgeIndex(current, len) + 1) % len,
        };
      });
    }, QUOTA_BADGE_ROTATE_MS);
    return () => window.clearInterval(id);
  }, [rotate, amberRows.length, signature]);

  const tooltip = useMemo(() => buildTooltip(amberRows, t), [amberRows, t]);
  const listForAria = amberRows.map((row) => formatListItem(row, t)).join(", ");
  const ariaLabel =
    amberRows.length === 0
      ? t("quotaBadgeAriaOk")
      : t("quotaBadgeAria", { count: amberRows.length, list: listForAria });

  // aria-live announces when this string changes; unchanged ticks do not re-announce.
  const liveText =
    amberRows.length === 0
      ? t("quotaOk")
      : t("quotaBadge", {
          tool: quotaBadgeName(active!),
          percent: quotaBadgePercent(active!),
        });

  const tone: QuotaBadgeTone = active ? quotaBadgeTone(active) : "ok";
  const moreCount = Math.max(0, amberRows.length - 1);

  const pause = () => setPaused(true);
  const resume = () => setPaused(false);
  const onMouseEnter: MouseEventHandler<HTMLAnchorElement> = pause;
  const onMouseLeave: MouseEventHandler<HTMLAnchorElement> = resume;
  const onMouseOver: MouseEventHandler<HTMLAnchorElement> = pause;
  const onMouseOut: MouseEventHandler<HTMLAnchorElement> = (event) => {
    const next = event.relatedTarget as Node | null;
    if (next && event.currentTarget.contains(next)) {
      return;
    }
    resume();
  };
  const onFocus: FocusEventHandler<HTMLAnchorElement> = pause;
  const onBlur: FocusEventHandler<HTMLAnchorElement> = resume;

  const body =
    amberRows.length === 0
      ? createElement("span", { className: "truncate" }, t("quotaOk"))
      : createElement(
          Fragment,
          null,
          createElement(
            "span",
            { className: "flex min-w-0 flex-1 items-baseline gap-1 overflow-hidden" },
            createElement(
              "span",
              { lang: "en", className: "min-w-0 truncate normal-case" },
              quotaBadgeName(active!),
            ),
            createElement(
              "span",
              { className: "tnum shrink-0" },
              t("quotaBadgePercent", { percent: quotaBadgePercent(active!) }),
            ),
          ),
          reducedMotion && moreCount > 0
            ? createElement(
                "span",
                {
                  "data-testid": "quota-badge-more",
                  className: "tnum shrink-0 rounded border border-current/30 px-1 text-meta",
                },
                t("quotaBadgeMore", { count: moreCount }),
              )
            : null,
        );

  return createElement(
    LinkComponent,
    {
      href: "/quotas",
      title: tooltip,
      "aria-label": ariaLabel,
      "data-testid": "quota-header-badge",
      "data-amber-count": String(amberRows.length),
      "data-badge-index": String(safeIndex),
      "data-paused": paused ? "1" : "0",
      onMouseEnter,
      onMouseLeave,
      onMouseOver,
      onMouseOut,
      onFocus,
      onBlur,
      className: `flex min-w-0 max-w-[7.5rem] items-center gap-1 rounded border px-2 py-0.5 font-mono text-body xl:max-w-[14rem] ${toneClass(tone)}`,
    },
    createElement("span", {
      className: `h-1.5 w-1.5 shrink-0 rounded-full ${toneDotClass(tone)}`,
    }),
    createElement(
      "span",
      { className: "flex min-w-0 flex-1 items-center gap-1 overflow-hidden" },
      body,
    ),
    createElement("span", { className: "sr-only", "aria-live": "polite" }, liveText),
  );
}
