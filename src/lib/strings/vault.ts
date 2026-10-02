/** Vault UI strings via react-i18next (PR-5). */
import { i18n, initI18n } from "@/lib/i18n/config";

function t(key: string, opts?: Record<string, unknown>): string {
  initI18n();
  return i18n.t(key, { ns: "vault", ...opts });
}

export const vaultStrings = {
  get experienceLog() {
    return t("experienceLog");
  },
  get semanticMapTitle() {
    return t("semanticMapTitle");
  },
  get deadSymbols() {
    return t("deadSymbols");
  },
  get clean() {
    return t("clean");
  },
  warnings: (n: number) => t("warnings", { count: n }),
  noDeadForNode: (name: string) => t("noDeadForNode", { name }),
  noExperiencesForNode: (name: string) => t("noExperiencesForNode", { name }),
  get noExperiences() {
    return t("noExperiences");
  },
  get showArchived() {
    return t("showArchived");
  },
  get hideArchived() {
    return t("hideArchived");
  },
  get archivedLabel() {
    return t("archivedLabel");
  },
  get unreviewed() {
    return t("unreviewed");
  },
  get markReviewed() {
    return t("markReviewed");
  },
  get markAllReviewed() {
    return t("markAllReviewed");
  },
  get edit() {
    return t("edit");
  },
  get save() {
    return t("save");
  },
  get cancel() {
    return t("cancel");
  },
  get archive() {
    return t("archive");
  },
  get unarchive() {
    return t("unarchive");
  },
  get pin() {
    return t("pin");
  },
  get unpin() {
    return t("unpin");
  },
  get restoreTtl() {
    return t("restoreTtl");
  },
  get revert() {
    return t("revert");
  },
  get loading() {
    return t("loading");
  },
  get loadError() {
    return t("loadError");
  },
  get listError() {
    return t("listError");
  },
  get archiveConfirm() {
    return t("archiveConfirm");
  },
  get confirmArchive() {
    return t("confirmArchive");
  },
  get confirmCancel() {
    return t("confirmCancel");
  },
  get originalContent() {
    return t("originalContent");
  },
  get metadata() {
    return t("metadata");
  },
  get useCount() {
    return t("useCount");
  },
  get lastUsed() {
    return t("lastUsed");
  },
  get updated() {
    return t("updated");
  },
  get source() {
    return t("source");
  },
  get relatedTask() {
    return t("relatedTask");
  },
  get tags() {
    return t("tags");
  },
  get outcome() {
    return t("outcome");
  },
  get project() {
    return t("project");
  },
  get projectPath() {
    return t("projectPath");
  },
  get agent() {
    return t("agent");
  },
  get adr() {
    return t("adr");
  },
  get loadMore() {
    return t("loadMore");
  },
  totalExperiences: (n: number) => t("totalExperiences", { count: n }),
  astNodes: (n: number) => t("astNodes", { count: n }),
  edges: (n: number) => t("edges", { count: n }),
  files: (n: number) => t("files", { count: n }),
  get useful() {
    return t("useful");
  },
  get usefulDone() {
    return t("usefulDone");
  },
  get whisper() {
    return t("whisper");
  },
  get synced() {
    return t("synced");
  },
  filter: (name: string) => t("filter", { name }),
  whisperLive: (n: number) => t("whisperLive", { count: n }),
} as const;
