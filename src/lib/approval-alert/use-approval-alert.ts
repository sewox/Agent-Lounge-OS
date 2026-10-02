"use client";

import { useEffect, useState } from "react";
import { ApprovalAlertEngine } from "./engine";
import { APPROVAL_SOUND_STORAGE_KEY } from "./settings";

export function useApprovalAlert(isPending: boolean): void {
  const [engine] = useState(() => new ApprovalAlertEngine());

  useEffect(() => {
    if (isPending) {
      engine.onPending();
    } else {
      engine.onResolved();
    }
  }, [engine, isPending]);

  // Apply interval/volume changes while an alert is already pending.
  useEffect(() => {
    const refresh = () => {
      engine.refreshSettings();
    };
    const onStorage = (event: StorageEvent) => {
      if (event.key === APPROVAL_SOUND_STORAGE_KEY || event.key === "approval-sound") {
        refresh();
      }
    };
    window.addEventListener("storage", onStorage);
    window.addEventListener("lounge.approvalSound", refresh);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("lounge.approvalSound", refresh);
    };
  }, [engine]);

  useEffect(() => {
    return () => {
      engine.dispose();
    };
  }, [engine]);
}
