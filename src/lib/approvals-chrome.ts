/**
 * Approval chrome affordances shared by the shell banner and health badges.
 * Icon names here are the glyphs the approval UI actually renders.
 */

import type { ApprovalKind } from "@/lib/lounge";

/** Icon used next to `source_unverified` approval chrome (warn triangle). */
export const SOURCE_UNVERIFIED_APPROVAL_ICON = "warn" as const;

/**
 * Icon name for an approval kind, matching what the shell approval banner
 * renders via `<Icon data-qa="approval-kind-icon" />`.
 */
export function approvalKindIcon(kind: ApprovalKind | string | undefined): string {
  if (kind === "source_unverified") {
    return SOURCE_UNVERIFIED_APPROVAL_ICON;
  }
  // Other kinds use text-only chrome today; warn remains the generic glyph.
  return SOURCE_UNVERIFIED_APPROVAL_ICON;
}
