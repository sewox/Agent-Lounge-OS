"use client";

import { useEffect, useState } from "react";
import { ApprovalAlertEngine } from "./engine";

export function useApprovalAlert(isPending: boolean): void {
  const [engine] = useState(() => new ApprovalAlertEngine());

  useEffect(() => {
    if (isPending) {
      engine.onPending();
    } else {
      engine.onResolved();
    }
  }, [engine, isPending]);

  useEffect(() => {
    return () => {
      engine.dispose();
    };
  }, [engine]);
}
