"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  explainJournalWhy,
  formatJournalDurationMs,
  queryEventJournal,
  shortJournalHash,
  verifyEventJournal,
  type JournalEntry,
  type JournalVerificationReport,
  type JournalWhyReport,
} from "@/lib/lounge";
import { useIsTauri } from "@/hooks/use-is-tauri";

type JournalCategory = "all" | "task" | "experience" | "control";

function categoryPrefix(cat: JournalCategory): string | null {
  if (cat === "task") return "task.";
  if (cat === "experience") return "experience.";
  if (cat === "control") return "control.";
  return null;
}

function isApprovalPhase(actionType: string): boolean {
  return (
    actionType === "task.approval_pending" ||
    actionType === "task.approval_approved" ||
    actionType === "task.approval_rejected"
  );
}

function isExecutionPhase(actionType: string): boolean {
  return (
    actionType.startsWith("task.") &&
    !isApprovalPhase(actionType) &&
    actionType !== "task.requested"
  );
}

export function JournalPanel({ embedded = false }: { embedded?: boolean }) {
  const { t, i18n } = useTranslation("stream");
  const tauriReady = useIsTauri();
  const locale = i18n.language || "en";
  const [entries, setEntries] = useState<JournalEntry[]>([]);
  const [verification, setVerification] = useState<JournalVerificationReport | null>(null);
  const [category, setCategory] = useState<JournalCategory>("all");
  const [why, setWhy] = useState<JournalWhyReport | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!tauriReady) {
      setEntries([]);
      setVerification(null);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const [rows, report] = await Promise.all([
        queryEventJournal({ limit: 200 }),
        verifyEventJournal(),
      ]);
      setEntries(rows);
      setVerification(report);
    } catch (err) {
      setError(err instanceof Error ? err.message : t("journalLoadError"));
    } finally {
      setLoading(false);
    }
  }, [tauriReady, t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const filtered = useMemo(() => {
    const prefix = categoryPrefix(category);
    if (!prefix) return entries;
    return entries.filter((row) => row.actionType.startsWith(prefix));
  }, [category, entries]);

  const openWhy = useCallback(
    async (targetId: string) => {
      setSelectedId(targetId);
      try {
        const report = await explainJournalWhy(targetId);
        setWhy(report);
      } catch (err) {
        setError(err instanceof Error ? err.message : t("journalLoadError"));
      }
    },
    [t],
  );

  const shell = embedded
    ? "flex w-full min-w-0 flex-col overflow-hidden bg-surface-container"
    : "flex w-full min-w-0 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container";

  const approvalMs = why?.approvalWaitMs ?? 0;
  const executionMs = why?.executionMs ?? 0;
  const totalMs = why?.totalDurationMs ?? approvalMs + executionMs;
  const approvalPct =
    totalMs > 0 ? Math.max(4, Math.round((approvalMs / totalMs) * 100)) : 0;
  const executionPct =
    totalMs > 0 ? Math.max(4, Math.round((executionMs / Math.max(totalMs, 1)) * 100)) : 0;

  return (
    <section data-qa="panel" data-journal-panel className={shell}>
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-outline-variant bg-surface-container-low p-2.5">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h2 className="font-body text-panel font-semibold tracking-label text-on-surface">
            {t("journalTitle")}
          </h2>
          {verification ? (
            verification.valid ? (
              <span
                data-qa="journal-chain-badge"
                data-valid="true"
                className="inline-flex max-w-full flex-wrap items-center gap-1.5 rounded border border-secondary/40 bg-secondary-container/30 px-2 py-0.5 font-body text-meta text-secondary"
              >
                <span>
                  {t("journalChainVerified", { count: verification.checkedCount })}
                </span>
                {verification.headHash ? (
                  <span lang="en" className="font-mono text-meta text-on-surface-variant">
                    {shortJournalHash(verification.headHash)}
                  </span>
                ) : null}
              </span>
            ) : (
              <span
                data-qa="journal-chain-badge"
                data-valid="false"
                className="inline-flex max-w-full flex-wrap items-center gap-1.5 rounded border border-error-container bg-error-container/40 px-2 py-0.5 font-body text-meta text-error-dim"
              >
                <span data-qa="journal-chain-broken">
                  {t("journalChainBroken", {
                    seq: verification.brokenAtSeq ?? "—",
                    id: verification.brokenEventId ?? "—",
                  })}
                </span>
              </span>
            )
          ) : null}
        </div>
        <div
          className="flex flex-wrap items-center rounded border border-outline-variant/70 bg-surface-container-high p-0.5 font-body text-meta"
          role="group"
          aria-label={t("journalFiltersLabel")}
        >
          {(
            [
              ["all", "journalFilterAll", "journal-filter-all"],
              ["task", "journalFilterTask", "journal-filter-task"],
              ["experience", "journalFilterExperience", "journal-filter-experience"],
              ["control", "journalFilterControl", "journal-filter-control"],
            ] as const
          ).map(([key, labelKey, qa]) => (
            <button
              key={key}
              type="button"
              data-qa={qa}
              aria-pressed={category === key}
              onClick={() => setCategory(key)}
              className={`min-h-8 rounded px-2.5 py-1 ${
                category === key
                  ? "bg-primary-container font-medium text-on-primary-container"
                  : "text-on-surface-variant hover:text-on-surface"
              }`}
            >
              {key === "task" || key === "experience" || key === "control" ? (
                <span lang="en">{t(labelKey)}</span>
              ) : (
                t(labelKey)
              )}
            </button>
          ))}
        </div>
      </div>

      <div className="grid min-h-0 grid-cols-1 gap-0 lg:grid-cols-2">
        <div className="min-h-0 max-h-[18rem] overflow-auto border-b border-outline-variant/40 lg:border-b-0 lg:border-r">
          {loading ? (
            <p className="px-2.5 py-4 font-body text-body text-outline" data-qa="layout-empty">
              {t("journalLoading")}
            </p>
          ) : null}
          {error ? (
            <p className="px-2.5 py-4 font-body text-body text-error" data-qa="layout-empty">
              {error}
            </p>
          ) : null}
          {!loading && !error && filtered.length === 0 ? (
            <p className="px-2.5 py-4 font-body text-body text-outline" data-qa="layout-empty">
              {t("journalEmpty")}
            </p>
          ) : null}
          <ul className="divide-y divide-outline-variant/30" data-qa="journal-entry-list">
            {filtered.map((row) => {
              const selected =
                selectedId === row.eventId ||
                (row.taskId != null && selectedId === row.taskId);
              return (
                <li key={`${row.seq}-${row.eventId}`}>
                  <button
                    type="button"
                    data-qa="journal-entry-row"
                    data-action-type={row.actionType}
                    data-task-id={row.taskId ?? ""}
                    onClick={() => {
                      void openWhy(row.taskId || row.eventId);
                    }}
                    className={`flex w-full min-w-0 flex-wrap items-center gap-2 px-2.5 py-1.5 text-left font-body text-body hover:bg-surface-container-high ${
                      selected ? "bg-primary-container/20" : ""
                    }`}
                  >
                    <span lang="en" className="tnum font-mono text-meta text-outline">
                      #{row.seq}
                    </span>
                    <span lang="en" className="min-w-0 break-words font-mono text-meta text-primary">
                      {row.actionType}
                    </span>
                    <span lang="en" className="font-mono text-meta text-on-surface-variant">
                      {row.actor}
                    </span>
                    <span
                      lang="en"
                      className="ml-auto rounded border border-outline-variant/60 bg-surface-container-high px-1.5 py-0.5 font-mono text-meta text-outline"
                    >
                      {shortJournalHash(row.entryHash)}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </div>

        <div className="min-h-0 max-h-[18rem] overflow-auto p-2.5" data-qa="journal-why-pane">
          {!why ? (
            <p className="font-body text-body text-outline" data-qa="layout-empty">
              {t("journalSelectHint")}
            </p>
          ) : (
            <div className="space-y-3">
              <div
                data-qa="journal-why-metrics"
                className="flex flex-wrap items-center gap-2 font-body text-meta"
              >
                <span className="rounded border border-outline-variant bg-surface-container-high px-2 py-0.5 text-on-surface">
                  {t("journalTotalDuration", {
                    duration: formatJournalDurationMs(why.totalDurationMs, locale),
                  })}
                </span>
                <span
                  data-qa="journal-approval-wait"
                  className="rounded border border-tertiary/50 bg-surface-container-highest px-2 py-0.5 text-tertiary"
                >
                  {t("journalApprovalWait", {
                    duration: formatJournalDurationMs(why.approvalWaitMs, locale),
                  })}
                </span>
                <span
                  data-qa="journal-execution-time"
                  className="rounded border border-primary/40 bg-primary-container/25 px-2 py-0.5 text-primary"
                >
                  {t("journalExecution", {
                    duration: formatJournalDurationMs(why.executionMs, locale),
                  })}
                </span>
              </div>

              <div data-qa="journal-phase-timeline" className="space-y-2">
                <div
                  className="flex h-2.5 w-full overflow-hidden rounded bg-surface-container-high"
                  aria-hidden
                >
                  {approvalPct > 0 ? (
                    <div
                      className="h-full bg-tertiary/70"
                      style={{ width: `${approvalPct}%` }}
                      title={t("journalApprovalWait", {
                        duration: formatJournalDurationMs(why.approvalWaitMs, locale),
                      })}
                    />
                  ) : null}
                  {executionPct > 0 ? (
                    <div
                      className="h-full bg-primary/70"
                      style={{ width: `${executionPct}%` }}
                      title={t("journalExecution", {
                        duration: formatJournalDurationMs(why.executionMs, locale),
                      })}
                    />
                  ) : null}
                </div>
                <ol className="space-y-1.5">
                  {why.events.map((row) => {
                    const phase = isApprovalPhase(row.actionType)
                      ? "approval"
                      : isExecutionPhase(row.actionType)
                        ? "execution"
                        : "other";
                    return (
                      <li
                        key={`why-${row.seq}-${row.eventId}`}
                        data-qa="journal-phase-step"
                        data-phase={phase}
                        className={`flex min-w-0 flex-wrap items-center gap-2 rounded border px-2 py-1 font-body text-meta ${
                          phase === "approval"
                            ? "border-tertiary/40 bg-surface-container-highest"
                            : phase === "execution"
                              ? "border-primary/30 bg-primary-container/15"
                              : "border-outline-variant/50 bg-surface-container-high/40"
                        }`}
                      >
                        <span lang="en" className="tnum font-mono text-outline">
                          #{row.seq}
                        </span>
                        <span lang="en" className="font-mono text-on-surface">
                          {row.actionType}
                        </span>
                        <span lang="en" className="font-mono text-on-surface-variant">
                          {row.actor}
                        </span>
                        <span lang="en" className="ml-auto font-mono text-outline">
                          {shortJournalHash(row.entryHash)}
                        </span>
                      </li>
                    );
                  })}
                </ol>
              </div>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
