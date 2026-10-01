"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { onAction } from "@tauri-apps/plugin-notification";
import { useEffect, useRef } from "react";
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

function taskIdFromNotificationExtra(extra: Record<string, unknown> | undefined): string | null {
  if (!extra) {
    return null;
  }
  const raw = extra.task_id ?? extra.taskId;
  return typeof raw === "string" && raw.length > 0 ? raw : null;
}

/**
 * Desktop notification click → focus main window + approval banner (AP-10).
 * Uses `@tauri-apps/plugin-notification` `onAction` where the OS delivers clicks;
 * when click events are unavailable, falls back to focusing the banner on the next
 * window/visibility focus while a pending approval exists.
 */
export function ApprovalNotificationBridge() {
  const { approval } = useLounge();
  const taskId = approval?.task_id;
  const pendingTaskIdRef = useRef<string | undefined>(undefined);

  useEffect(() => {
    pendingTaskIdRef.current = taskId;
  }, [taskId]);

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

      try {
        const actionListener = await onAction((notification) => {
          const fromExtra = taskIdFromNotificationExtra(
            notification.extra as Record<string, unknown> | undefined,
          );
          const id = fromExtra ?? pendingTaskIdRef.current ?? null;
          void invoke("focus_app_for_approval", { taskId: id }).catch(() => {
            focusApprovalBanner(id ?? undefined);
          });
        });
        unlisteners.push(() => {
          void actionListener.unregister();
        });
      } catch {
        /* notification action API unavailable on this platform / harness */
      }
    })();

    return () => {
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  // Fallback when OS cannot deliver click events: next window focus with a pending approval.
  useEffect(() => {
    if (!isTauri()) {
      return;
    }
    const onFocus = () => {
      const id = pendingTaskIdRef.current;
      if (id) {
        focusApprovalBanner(id);
      }
    };
    const onVisibility = () => {
      if (!document.hidden) {
        onFocus();
      }
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
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
      data-platform-note="Native OS notification click → focus_app_for_approval via plugin onAction; window-focus fallback when click events are unavailable (AP-10)."
    />
  );
}
