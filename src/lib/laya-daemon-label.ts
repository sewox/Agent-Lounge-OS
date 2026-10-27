import type { DecisionGateStatus } from "@/lib/lounge";

type ShellT = (key: string, options?: Record<string, unknown>) => string;

/**
 * Sidebar Laya status label.
 * When ready: always `${running} · ${device}` (never latency — shown in Telemetry/Stream).
 */
export function layaDaemonLabel(
  status: DecisionGateStatus | null,
  t: ShellT,
): string {
  if (!status) {
    return t("disconnected");
  }
  const phase = status.phase as string;
  if (phase === "ready") {
    const device = typeof status.device === "string" ? status.device.trim() : "";
    if (device) {
      return `${t("running")} · ${device}`;
    }
    return t("running");
  }
  if (phase === "checking" || phase === "downloading" || phase === "loading") {
    const progress =
      status && "progress" in status
        ? String((status as { progress?: unknown }).progress ?? "")
        : "";
    return progress || t("checking");
  }
  if (phase === "recommended" || phase === "idle" || phase === "available") {
    return t("recommended");
  }
  if (phase === "error" || phase === "failed") {
    const err =
      status.reason ||
      ("error" in status ? String((status as { error?: unknown }).error ?? "") : "");
    return err || t("errorShort");
  }
  return t("disconnected");
}
