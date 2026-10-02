"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { onAction } from "@tauri-apps/plugin-notification";
import { useEffect, useRef } from "react";
import { useLounge } from "@/components/lounge-provider";
import { useIsTauri } from "@/hooks/use-is-tauri";

const APPROVAL_BANNER_FOCUS_EVENT = "approval_banner_focus";

function focusApprovalBanner(taskId?: string): void {
  const scopedChrome = taskId
    ? document.querySelector<HTMLElement>(`[data-approval-chrome][data-task-id="${taskId}"]`)
    : null;
  // Always focus the tabindex=-1 banner node — outer chrome has no tabIndex.
  const banner =
    scopedChrome?.querySelector<HTMLElement>('[data-qa="approval-banner"]') ??
    document.querySelector<HTMLElement>('[data-qa="approval-banner"]') ??
    document.querySelector<HTMLElement>('[data-approval-chrome="routing"] [tabindex]') ??
    document.querySelector<HTMLElement>('[data-approval-chrome="security"]') ??
    document.querySelector<HTMLElement>('[data-approval-chrome="quota"]');
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
 * AP-10: OS notification / app activation → focus main window + approval banner.
 *
 * Desktop reality (`@tauri-apps/plugin-notification` guest-js): `onAction` is
 * **mobile-only** (action types). Win/mac/Linux never deliver it for our toasts.
 * Desktop path is Rust-side: dock/taskbar attention + `RunEvent::Reopen` (macOS) /
 * `WindowEvent::Focused(true)` while a pending approval is recorded →
 * `focus_app_for_approval` → `approval_banner_focus`. See
 * `docs/qa/ap-10-notification-click.md`.
 */
export function ApprovalNotificationBridge() {
  const { approval } = useLounge();
  const taskId = approval?.task_id;
  const pendingTaskIdRef = useRef<string | undefined>(undefined);
  const tauriReady = useIsTauri();
  const listenersReadyRef = useRef(false);

  useEffect(() => {
    pendingTaskIdRef.current = taskId;
  }, [taskId]);

  useEffect(() => {
    if (!tauriReady) {
      return;
    }

    const unlisteners: Array<() => void> = [];
    let cancelled = false;

    void (async () => {
      try {
        unlisteners.push(
          await listen<{ task_id?: string }>(APPROVAL_BANNER_FOCUS_EVENT, (event) => {
            focusApprovalBanner(event.payload?.task_id);
          }),
        );
        if (!cancelled) {
          listenersReadyRef.current = true;
          document
            .querySelector('[data-qa="approval-notification"]')
            ?.setAttribute("data-listeners-ready", "1");
        }
      } catch (err) {
        console.warn("[ap-10] approval_banner_focus listen failed:", err);
      }

      // Mobile-only API. Register for completeness; log failures — never swallow.
      try {
        const actionListener = await onAction((notification) => {
          const fromExtra = taskIdFromNotificationExtra(
            notification.extra as Record<string, unknown> | undefined,
          );
          const id = fromExtra ?? pendingTaskIdRef.current ?? null;
          void invoke("focus_app_for_approval", { taskId: id }).catch((err) => {
            console.warn("[ap-10] focus_app_for_approval (onAction) failed:", err);
            focusApprovalBanner(id ?? undefined);
          });
        });
        unlisteners.push(() => {
          void actionListener.unregister();
        });
      } catch (err) {
        console.info(
          "[ap-10] notification onAction unavailable (expected on desktop; mobile-only):",
          err,
        );
      }
    })();

    return () => {
      cancelled = true;
      listenersReadyRef.current = false;
      unlisteners.forEach((fn) => fn());
    };
  }, [tauriReady]);

  // Fallback when the OS activates the window without a click payload: next
  // window/visibility focus while a pending approval exists asks Rust to raise
  // + emit approval_banner_focus (also covers the Playwright harness).
  useEffect(() => {
    if (!tauriReady) {
      return;
    }
    const requestFocus = () => {
      const id = pendingTaskIdRef.current;
      if (!id) {
        return;
      }
      void invoke("focus_app_for_approval", { taskId: id }).catch((err) => {
        console.warn("[ap-10] focus_app_for_approval (window-focus fallback) failed:", err);
        focusApprovalBanner(id);
      });
    };
    const onVisibility = () => {
      if (!document.hidden) {
        requestFocus();
      }
    };
    window.addEventListener("focus", requestFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", requestFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [tauriReady]);

  useEffect(() => {
    if (taskId) {
      focusApprovalBanner(taskId);
    }
  }, [taskId]);

  // Always render the marker (SSR + first client paint identical).
  return (
    <span
      data-qa="approval-notification"
      hidden
      aria-hidden
      data-platform-note="Desktop: Rust Reopen/Focused + dock attention → focus_app_for_approval. Plugin onAction is mobile-only. See docs/qa/ap-10-notification-click.md."
      data-tauri-ready={tauriReady ? "1" : "0"}
      data-listeners-ready="0"
    />
  );
}
