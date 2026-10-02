"use client";

import { useLounge } from "@/components/lounge-provider";
import { useApprovalAlert } from "@/lib/approval-alert/use-approval-alert";

/** Wires the HTML Audio alert loop to pending approval state (AP-08). */
export function ApprovalAlertBridge() {
  const { approval, destructiveQueue } = useLounge();
  useApprovalAlert(Boolean(approval) || destructiveQueue.length > 0);
  return null;
}
