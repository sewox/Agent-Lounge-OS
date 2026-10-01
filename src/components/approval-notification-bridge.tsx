"use client";

import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useLounge } from "@/components/lounge-provider";
import { isTauri } from "@/lib/lounge";

const APPROVAL_BANNER_FOCUS_EVENT = "approval_banner_focus";

function focusApprovalBanner(taskId?: string): void {
  const banner =
    document.querySelector<HTMLElement>('[data-qa="approval-banner"]') ??
    document.querySelector<HTMLElement>('[data-approval-chrome="routing"]') ??
    document.querySelector<HTMLElement>('[data-approval-chrome="security"]') ??
    document.querySelector<HTMLElement>('[data-approval-chrome="quota"]');
  if (taskId) {
    const scoped = document.querySelector<HTMLElement>(
      `[data-approval-chrome][data-task-id="${taskId}"]`,
    );
    (scoped ?? banner)?.scrollIntoView({ block: "nearest" });
    (scoped ?? banner)?.focus({ preventScroll: true });
    return;
  }
  banner?.scrollIntoView({ block: "nearest" });
  banner?.focus({ preventScroll: true });
}

/**
 * Desktop notification click → focus main window + approval banner (AP-10).
 * Some platforms cannot deliver click events; window focus + Rust emit is the fallback.
 */
export function ApprovalNotificationBridge() {
  const { approval } = useLounge();
  const taskId = approval?.task_id;

  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    const unlisteners: Array<() => void> = [];

    void (async () => {
      try {
        unlisteners.push(
          await listen<{ task_id?: string }>(APPROVAL_BANNER_FOCUS_EVENT, (event) => {
            focusApprovalBanner(event.payload?.task_id);
          }),
        );
      } catch {
        /* event plugin unavailable in harness */
      }
    })();

    return () => {
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (taskId) {
      focusApprovalBanner(taskId);
    }
  }, [taskId]);

  if (!isTauri()) {
    return null;
  }

  return (
    <span
      data-qa="approval-notification"
      hidden
      aria-hidden
      data-platform-note="Native OS notification click is verified manually on macOS, Windows, and Linux (AP-10)."
    />
  );
}
