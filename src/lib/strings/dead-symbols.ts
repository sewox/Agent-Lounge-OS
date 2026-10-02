/** Dead-symbols / health UI strings via react-i18next (PR-5). */
import { i18n, initI18n } from "@/lib/i18n/config";

function t(key: string, opts?: Record<string, unknown>): string {
  initI18n();
  return i18n.t(key, { ns: "health", ...opts });
}

export const deadSymbolStrings = {
  get panelTitle() {
    return t("panelTitle");
  },
  get ignoreListTitle() {
    return t("ignoreListTitle");
  },
  get healthTab() {
    return t("healthTab");
  },
  get deadTab() {
    return t("deadTab");
  },
  get ignoreTab() {
    return t("ignoreTab");
  },
  get searchPlaceholder() {
    return t("searchPlaceholder");
  },
  get kindAll() {
    return t("kindAll");
  },
  get kindUnused() {
    return t("kindUnused");
  },
  get kindBroken() {
    return t("kindBroken");
  },
  get sortName() {
    return t("sortName");
  },
  get sortFile() {
    return t("sortFile");
  },
  get sortKind() {
    return t("sortKind");
  },
  total: (n: number) => t("total", { count: n }),
  get noSymbols() {
    return t("noSymbols");
  },
  get noIgnored() {
    return t("noIgnored");
  },
  get selectSymbol() {
    return t("selectSymbol");
  },
  get lastRef() {
    return t("lastRef");
  },
  get noLastRef() {
    return t("noLastRef");
  },
  get project() {
    return t("project");
  },
  get fileLine() {
    return t("fileLine");
  },
  get detail() {
    return t("detail");
  },
  get openInEditor() {
    return t("openInEditor");
  },
  get copyPath() {
    return t("copyPath");
  },
  get ignore() {
    return t("ignore");
  },
  get unignore() {
    return t("unignore");
  },
  get fixWithAgent() {
    return t("fixWithAgent");
  },
  taskQueued: (msg: string) => t("taskQueued", { message: msg }),
  get noAgent() {
    return t("noAgent");
  },
  viewAll: (n: number) => t("viewAll", { count: n }),
  get reindex() {
    return t("reindex");
  },
  deadDrillDown: (n: number) => t("deadDrillDown", { count: n }),
} as const;
