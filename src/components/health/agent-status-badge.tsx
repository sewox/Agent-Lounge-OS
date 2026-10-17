"use client";

import { Icon } from "@/components/icons";
import { StatusPill } from "@/components/design-system";
import type { AgentMcpStatus } from "@/lib/lounge";
import { severityToTone, statusIconName } from "@/lib/health-agents";

type AgentStatusBadgeProps = {
  status: AgentMcpStatus;
  severity: string;
  label: string;
  className?: string;
};

/**
 * Status badge: colour from severity, text label always present (not colour-alone),
 * distinct icon per status (unverified ≠ source_unverified approval).
 */
export function AgentStatusBadge({
  status,
  severity,
  label,
  className = "",
}: AgentStatusBadgeProps) {
  const tone = severityToTone(severity);
  const icon = statusIconName(status);
  return (
    <StatusPill tone={tone} className={className}>
      <span data-qa="agent-status-icon" data-icon={icon} aria-hidden className="inline-flex">
        <Icon name={icon} className="h-3 w-3" />
      </span>
      <span data-qa="agent-status-label" data-status={status}>
        {label}
      </span>
    </StatusPill>
  );
}
