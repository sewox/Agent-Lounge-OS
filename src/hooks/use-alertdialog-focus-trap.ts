"use client";

import { useLayoutEffect, useRef, type RefObject } from "react";

const FOCUSABLE_SELECTOR =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Focusable descendants inside a dialog root (enabled, tabbable). */
export function listFocusable(root: HTMLElement | null): HTMLElement[] {
  if (!root) {
    return [];
  }
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)].filter(
    (node) => !node.hasAttribute("disabled") && node.tabIndex !== -1,
  );
}

/**
 * Prefer `initialFocusSelector` when it matches a focusable node; otherwise the
 * first focusable descendant.
 */
export function resolveInitialFocus(
  root: HTMLElement | null,
  initialFocusSelector?: string,
): HTMLElement | null {
  if (!root) {
    return null;
  }
  if (initialFocusSelector) {
    const preferred = root.querySelector<HTMLElement>(initialFocusSelector);
    if (preferred && listFocusable(root).includes(preferred)) {
      return preferred;
    }
  }
  return listFocusable(root)[0] ?? null;
}

/**
 * Tab / Shift+Tab wrap inside `root`. Returns true when the event was handled.
 * Safe to call from a capture-phase keydown listener installed synchronously
 * when the dialog opens (no setTimeout boot).
 */
export function handleAlertdialogTabKey(
  event: Pick<KeyboardEvent, "key" | "shiftKey" | "preventDefault">,
  root: HTMLElement | null,
  active: Element | null = typeof document !== "undefined" ? document.activeElement : null,
): boolean {
  if (event.key !== "Tab" || !root) {
    return false;
  }
  const focusable = listFocusable(root);
  if (focusable.length === 0) {
    return false;
  }
  const first = focusable[0]!;
  const last = focusable[focusable.length - 1]!;
  if (event.shiftKey && active === first) {
    event.preventDefault();
    last.focus();
    return true;
  }
  if (!event.shiftKey && active === last) {
    event.preventDefault();
    first.focus();
    return true;
  }
  return false;
}

export type AlertdialogFocusTrapOptions = {
  open: boolean;
  containerRef: RefObject<HTMLElement | null>;
  /** CSS selector for the preferred initial focus target inside the dialog. */
  initialFocusSelector?: string;
  onEscape?: () => void;
  /**
   * When false, Escape is ignored. Read on each keydown via a ref so the trap
   * effect does not re-subscribe (and must not re-run boot focus) when busy
   * toggles while the dialog stays open.
   */
  escapeEnabled?: boolean;
  /**
   * Optional identity for the dialog contents (e.g. destructive queue item id).
   * Changing it while `open` stays true re-installs the trap and re-applies
   * initial focus once — still synchronously, never via setTimeout.
   */
  instanceKey?: string | null;
};

/**
 * Synchronous alertdialog focus management:
 * - Initial focus + Tab trap are installed in `useLayoutEffect` (before paint).
 * - No `setTimeout` boot — avoids yanking focus after the user already Tabbed.
 * - Effect depends on `open` (+ optional `instanceKey`); busy/escape use refs.
 * - Cleanup restores focus to the trigger that opened the dialog.
 */
export function useAlertdialogFocusTrap({
  open,
  containerRef,
  initialFocusSelector,
  onEscape,
  escapeEnabled = true,
  instanceKey = null,
}: AlertdialogFocusTrapOptions): void {
  const onEscapeRef = useRef(onEscape);
  const escapeEnabledRef = useRef(escapeEnabled);
  const initialFocusSelectorRef = useRef(initialFocusSelector);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  onEscapeRef.current = onEscape;
  escapeEnabledRef.current = escapeEnabled;
  initialFocusSelectorRef.current = initialFocusSelector;

  useLayoutEffect(() => {
    if (!open) {
      return;
    }

    const previouslyFocused = document.activeElement;
    previousFocusRef.current =
      previouslyFocused && typeof (previouslyFocused as HTMLElement).focus === "function"
        ? (previouslyFocused as HTMLElement)
        : null;

    const root = containerRef.current;
    resolveInitialFocus(root, initialFocusSelectorRef.current)?.focus();

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        if (!escapeEnabledRef.current) {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        onEscapeRef.current?.();
        return;
      }
      handleAlertdialogTabKey(event, containerRef.current, document.activeElement);
    }

    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      previousFocusRef.current?.focus();
      previousFocusRef.current = null;
    };
  }, [open, instanceKey, containerRef]);
}
