"use client";

import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import { useTranslation } from "react-i18next";
import {
  PanelGrid,
  StatsStrip,
  StickyActionBar,
  type StatsStripItem,
} from "@/components/design-system";
import { useAlertdialogFocusTrap } from "@/hooks/use-alertdialog-focus-trap";
import { useLounge } from "@/components/lounge-provider";
import {
  formatExperienceTime,
  isTauri,
  type ExperienceOutcome,
  type LoungeExperience,
  type ReviewQueueFilter,
  type ReviewStatFilter,
  type ReviewStats,
} from "@/lib/lounge";
import {
  applyReviewFilters,
  assertCounterConsistency,
  experienceHasHygieneMask,
  maskedReviewContent,
  sanitizeReviewRow,
  type ReviewStatId,
} from "@/lib/vault-review";

type LoadState =
  | { phase: "browser" }
  | { phase: "loading" }
  | { phase: "error"; message: string }
  | { phase: "empty"; rows: LoungeExperience[]; stats: ReviewStats }
  | { phase: "ready"; rows: LoungeExperience[]; stats: ReviewStats };

type ConfirmKind = "approve" | "reject" | null;

function statToFilter(id: ReviewStatId): ReviewStatFilter {
  switch (id) {
    case "hygiene":
      return "hygiene";
    case "approved_today":
      return "approved_today";
    case "rejected":
      return "rejected";
    default:
      return "pending";
  }
}

export function VaultReviewPanel() {
  const { t } = useTranslation("vault");
  const { unreviewedCount, refreshUnreviewedCount, openExperience } = useLounge();
  const [state, setState] = useState<LoadState>(() =>
    isTauri() ? { phase: "loading" } : { phase: "browser" },
  );
  const [statId, setStatId] = useState<ReviewStatId>("pending");
  const [projectId, setProjectId] = useState("");
  const [agent, setAgent] = useState("");
  const [outcome, setOutcome] = useState("");
  const [hygieneOnly, setHygieneOnly] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [checked, setChecked] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<ConfirmKind>(null);
  const [editing, setEditing] = useState(false);
  const [editAdr, setEditAdr] = useState("");
  const [editTags, setEditTags] = useState("");
  const [editOutcome, setEditOutcome] = useState<ExperienceOutcome>("success");
  const listLabelId = useId();
  const confirmRef = useRef<HTMLDivElement | null>(null);
  const confirmTitleId = useId();

  useAlertdialogFocusTrap({
    open: confirm != null,
    containerRef: confirmRef,
    initialFocusSelector: '[data-qa="review-confirm-cancel"]',
    onEscape: () => {
      if (!busy) setConfirm(null);
    },
    escapeEnabled: !busy,
    instanceKey: confirm,
  });

  const load = useCallback(async () => {
    if (!isTauri()) {
      setState({ phase: "browser" });
      return;
    }
    setState({ phase: "loading" });
    setActionError(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const filter: ReviewQueueFilter = {
        stat: statToFilter(statId),
        project_id: projectId.trim() || null,
        agent: agent.trim() || null,
        outcome: outcome.trim() || null,
        hygiene_only: hygieneOnly,
        limit: 200,
      };
      const [rowsRaw, stats, canonicalPending] = await Promise.all([
        invoke<LoungeExperience[]>("list_review_queue", { filter }),
        invoke<ReviewStats>("count_review_stats"),
        invoke<number>("count_unreviewed_experiences"),
      ]);
      // Never carry pre-hygiene original_content into the review React tree.
      const rows = rowsRaw.map((row) => sanitizeReviewRow(row));
      // VR3: badge / StatsStrip pending / list share one counter source.
      if (!assertCounterConsistency(Number(canonicalPending), { pending: stats.pending })) {
        throw new Error(
          `counter drift: unreviewed=${canonicalPending} pending=${stats.pending}`,
        );
      }
      await refreshUnreviewedCount();
      const filtered = applyReviewFilters(rows, {
        projectId: projectId.trim() || null,
        agent: agent.trim() || null,
        outcome: outcome.trim() || null,
        hygieneOnly,
      });
      // Server already filtered; client re-apply is idempotent for unit-tested path.
      const list = filtered.length === rows.length ? rows : filtered;
      if (list.length === 0) {
        setState({ phase: "empty", rows: list, stats });
      } else {
        setState({ phase: "ready", rows: list, stats });
      }
    } catch (error) {
      setState({
        phase: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, [agent, hygieneOnly, outcome, projectId, refreshUnreviewedCount, statId]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void load();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [load]);

  const rows = useMemo(
    () => (state.phase === "ready" || state.phase === "empty" ? state.rows : []),
    [state],
  );
  const stats =
    state.phase === "ready" || state.phase === "empty" ? state.stats : null;

  const selected = useMemo(() => {
    if (rows.length === 0) return null;
    return rows.find((row) => row.id === selectedId) ?? rows[0] ?? null;
  }, [rows, selectedId]);

  const selectRow = useCallback((id: string) => {
    setSelectedId(id);
    setActionError(null);
    setEditing(false);
  }, []);

  const beginEdit = useCallback(() => {
    if (!selected) return;
    setEditAdr(selected.adr_summary ?? "");
    setEditTags((selected.tags ?? []).join(", "));
    setEditOutcome(selected.outcome);
    setEditing(true);
  }, [selected]);

  const moveSelection = useCallback(
    (delta: number) => {
      if (rows.length === 0) return;
      const current = selected ? rows.findIndex((r) => r.id === selected.id) : 0;
      const next = Math.max(0, Math.min(rows.length - 1, current + delta));
      selectRow(rows[next]!.id);
    },
    [rows, selected, selectRow],
  );

  const toggleChecked = useCallback((id: string) => {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const checkedIds = useMemo(() => [...checked], [checked]);

  const runApprove = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return;
      setBusy(true);
      setActionError(null);
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        if (ids.length === 1) {
          await invoke("mark_experience_reviewed", { id: ids[0] });
        } else {
          await invoke("mark_experiences_reviewed", { ids });
        }
        setChecked(new Set());
        setConfirm(null);
        await refreshUnreviewedCount();
        await load();
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setBusy(false);
      }
    },
    [load, refreshUnreviewedCount],
  );

  const runReject = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return;
      setBusy(true);
      setActionError(null);
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        if (ids.length === 1) {
          await invoke("reject_experience", { id: ids[0] });
        } else {
          await invoke("reject_experiences", { ids });
        }
        setChecked(new Set());
        setConfirm(null);
        await refreshUnreviewedCount();
        await load();
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setBusy(false);
      }
    },
    [load, refreshUnreviewedCount],
  );

  const runUndo = useCallback(
    async (id: string) => {
      setBusy(true);
      setActionError(null);
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("undo_reject_experience", { id });
        await refreshUnreviewedCount();
        await load();
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setBusy(false);
      }
    },
    [load, refreshUnreviewedCount],
  );

  const saveEdit = useCallback(
    async (event: FormEvent) => {
      event.preventDefault();
      if (!selected) return;
      setBusy(true);
      setActionError(null);
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const tags = editTags
          .split(",")
          .map((tag) => tag.trim())
          .filter(Boolean);
        await invoke("update_experience", {
          id: selected.id,
          patch: {
            adr_summary: editAdr,
            tags,
            outcome: editOutcome,
          },
        });
        setEditing(false);
        await load();
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setBusy(false);
      }
    },
    [editAdr, editOutcome, editTags, load, selected],
  );

  const statsItems: StatsStripItem[] = useMemo(() => {
    const s = stats ?? {
      pending: unreviewedCount,
      hygiene: 0,
      approved_today: 0,
      rejected: 0,
    };
    return [
      { id: "pending", label: t("review.statPending"), value: s.pending },
      { id: "hygiene", label: t("review.statHygiene"), value: s.hygiene },
      {
        id: "approved_today",
        label: t("review.statApprovedToday"),
        value: s.approved_today,
      },
      { id: "rejected", label: t("review.statRejected"), value: s.rejected },
    ];
  }, [stats, t, unreviewedCount]);

  const projectOptions = useMemo(
    () => [...new Set(rows.map((r) => r.project_id).filter(Boolean))].sort(),
    [rows],
  );
  const agentOptions = useMemo(
    () => [...new Set(rows.map((r) => r.agent).filter(Boolean))].sort(),
    [rows],
  );

  if (state.phase === "browser") {
    return (
      <section
        data-qa="panel"
        data-qa-vault-review="browser"
        className="flex h-full min-h-0 w-full flex-col items-center justify-center gap-2 rounded-lg border border-outline-variant bg-surface-container p-6 text-center"
      >
        <p className="max-w-md font-body text-body text-on-surface-variant">
          {t("review.browserMode")}
        </p>
      </section>
    );
  }

  if (state.phase === "loading") {
    return (
      <section
        data-qa="panel"
        data-qa-vault-review="loading"
        className="flex min-h-0 w-full min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
        aria-busy="true"
        aria-label={t("review.loading")}
      >
        <div className="shrink-0 border-b border-outline-variant bg-surface-container-low px-3 py-2">
          <h2 className="font-body text-panel font-semibold tracking-label uppercase">
            {t("review.listTitle")}
          </h2>
        </div>
        <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-hidden p-3">
          {Array.from({ length: 10 }, (_, i) => (
            <p
              key={i}
              className="min-h-0 flex-1 font-body text-body text-on-surface-variant"
            >
              {t("review.loading")}
            </p>
          ))}
        </div>
      </section>
    );
  }

  if (state.phase === "error") {
    return (
      <section
        data-qa="panel"
        data-qa-vault-review="error"
        className="flex h-full min-h-0 w-full flex-col items-center justify-center gap-3 rounded-lg border border-outline-variant bg-surface-container p-6 text-center"
      >
        <p className="max-w-lg font-body text-body text-error">
          {t("review.error", { message: state.message })}
        </p>
        <button
          type="button"
          data-qa="vault-review-retry"
          onClick={() => void load()}
          className="min-h-8 rounded border border-primary/40 px-3 py-1 font-body text-body text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
        >
          {t("retry")}
        </button>
      </section>
    );
  }

  const statsAndFilters = (
      <div className="shrink-0 space-y-2 border-b border-outline-variant bg-surface-container-low px-3 py-2">
        <h2
          id={listLabelId}
          className="font-body text-panel font-semibold tracking-label uppercase"
        >
          {t("review.listTitle")}
        </h2>
        <StatsStrip
          items={statsItems}
          activeId={statId}
          onFilterChange={(id) => {
            if (
              id === "pending" ||
              id === "hygiene" ||
              id === "approved_today" ||
              id === "rejected"
            ) {
              setStatId(id);
              setChecked(new Set());
            }
          }}
        />
        <div
          className="flex flex-wrap items-center gap-1.5"
          data-qa="vault-review-filters"
          role="group"
          aria-label={t("filters")}
        >
          <select
            data-qa="review-filter-project"
            value={projectId}
            onChange={(e) => setProjectId(e.target.value)}
            className="min-h-8 rounded border border-outline-variant bg-surface-container px-2 font-body text-meta"
            aria-label={t("project")}
          >
            <option value="">{t("review.filterProjectAll")}</option>
            {projectOptions.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          <select
            data-qa="review-filter-agent"
            value={agent}
            onChange={(e) => setAgent(e.target.value)}
            className="min-h-8 rounded border border-outline-variant bg-surface-container px-2 font-body text-meta"
            aria-label={t("agent")}
          >
            <option value="">{t("review.filterAgentAll")}</option>
            {agentOptions.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
          <select
            data-qa="review-filter-outcome"
            value={outcome}
            onChange={(e) => setOutcome(e.target.value)}
            className="min-h-8 rounded border border-outline-variant bg-surface-container px-2 font-body text-meta"
            aria-label={t("outcome")}
          >
            <option value="">{t("review.filterOutcomeAll")}</option>
            <option value="success">success</option>
            <option value="partial">partial</option>
            <option value="failure">failure</option>
          </select>
          <button
            type="button"
            data-qa="review-filter-hygiene"
            aria-pressed={hygieneOnly}
            onClick={() => setHygieneOnly((v) => !v)}
            className={`min-h-8 rounded border px-2 py-1 font-body text-meta focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary ${
              hygieneOnly
                ? "border-primary bg-primary-container text-on-primary-container"
                : "border-outline-variant bg-surface-container text-on-surface"
            }`}
          >
            {t("review.filterHygiene")}
          </button>
        </div>
        <p className="font-mono text-meta text-outline" data-qa="review-pending-count">
          {t("review.pendingCount", {
            count: stats?.pending ?? unreviewedCount,
          })}
        </p>
      </div>
  );

  if (state.phase === "empty") {
    // Single panel — honest empty (no three-slot empty frames / spacer inflation).
    // Fill remaining main height so L2/L3 occupancy matches Health ignore empty.
    return (
      <section
        data-qa="panel"
        data-qa-vault-review="empty"
        className="flex min-h-0 w-full min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-outline-variant bg-surface-container"
      >
        {statsAndFilters}
        <div
          data-qa="vault-review-empty"
          className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 px-4 py-8 text-center"
        >
          <p className="max-w-sm font-body text-body text-on-surface">{t("review.empty")}</p>
          <p className="max-w-sm font-body text-meta text-on-surface-variant">
            {t("review.emptyHint")}
          </p>
          <p className="max-w-md font-mono text-meta text-outline">
            {t("review.pendingCount", { count: 0 })}
          </p>
        </div>
      </section>
    );
  }

  const listPane = (
    <div className="flex h-full min-h-0 flex-col">
      {statsAndFilters}
        <div
          role="group"
          tabIndex={0}
          aria-labelledby={listLabelId}
          data-qa="vault-review-list"
          className="min-h-0 flex-1 overflow-auto font-mono text-body focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-primary"
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") {
              event.preventDefault();
              moveSelection(1);
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              moveSelection(-1);
            } else if (event.key === "Enter" && selected) {
              event.preventDefault();
              selectRow(selected.id);
            } else if (event.key === " " && selected) {
              event.preventDefault();
              toggleChecked(selected.id);
            }
          }}
        >
          {rows.map((row) => {
            const active = selected?.id === row.id;
            const hygiene = experienceHasHygieneMask(row);
            const isRejected = row.archived_by === "rejected";
            return (
              <div
                key={row.id}
                data-qa="vault-review-row"
                data-experience-id={row.id}
                data-hygiene={hygiene ? "true" : "false"}
                data-selected={active ? "true" : "false"}
                className={`flex items-start gap-2 border-b border-outline-variant/40 px-2 py-2 ${
                  active ? "bg-primary-container/30" : "hover:bg-surface-container-high"
                }`}
              >
                <input
                  type="checkbox"
                  data-qa="vault-review-check"
                  checked={checked.has(row.id)}
                  onChange={() => toggleChecked(row.id)}
                  aria-label={t("review.selectRow", { title: row.title || row.id })}
                  className="mt-1"
                />
                <button
                  type="button"
                  className="min-w-0 flex-1 text-left"
                  onClick={() => selectRow(row.id)}
                >
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="rounded border border-outline-variant/50 px-1 text-meta text-on-surface-variant">
                      {row.project_id}
                    </span>
                    {row.reviewed === false && !isRejected ? (
                      <span
                        data-qa="unreviewed-badge"
                        className="rounded border border-dashed border-secondary/60 px-1 text-meta text-secondary uppercase"
                      >
                        {t("unreviewed")}
                      </span>
                    ) : null}
                    {hygiene ? (
                      <span className="rounded border border-tertiary/50 px-1 text-meta text-tertiary">
                        {t("review.hygieneFlag")}
                      </span>
                    ) : null}
                    {isRejected ? (
                      <span className="rounded border border-error/40 px-1 text-meta text-error">
                        {t("review.rejectedLabel")}
                      </span>
                    ) : null}
                  </div>
                  <div className="mt-0.5 truncate font-body text-body text-on-surface">
                    {row.title || row.adr_summary || row.id}
                  </div>
                  <div className="font-mono text-meta text-outline">
                    {row.agent} · {formatExperienceTime(row.created_at)}
                  </div>
                </button>
              </div>
            );
          })}
        </div>

      {checkedIds.length > 0 && statId !== "rejected" && statId !== "approved_today" ? (
        <StickyActionBar
          label={t("review.bulkBar")}
          leading={t("review.selectedCount", { count: checkedIds.length })}
        >
          <button
            type="button"
            data-qa="review-bulk-approve"
            disabled={busy}
            onClick={() => setConfirm("approve")}
            className="min-h-8 rounded bg-primary-container px-3 py-1 font-body text-body font-semibold text-on-primary-container"
          >
            {t("review.approveSelected")}
          </button>
          <button
            type="button"
            data-qa="review-bulk-reject"
            disabled={busy}
            onClick={() => setConfirm("reject")}
            className="min-h-8 rounded border border-error/50 px-3 py-1 font-body text-body text-error"
          >
            {t("review.rejectSelected")}
          </button>
        </StickyActionBar>
      ) : null}
    </div>
  );

  const detailPane = (
    <div className="flex h-full min-h-0 flex-col" data-qa="vault-review-detail">
      {!selected ? (
        <p className="p-4 font-body text-body text-on-surface-variant">{t("review.noSelection")}</p>
      ) : editing ? (
        <form className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3" onSubmit={saveEdit}>
          <label className="block space-y-1">
            <span className="font-body text-meta uppercase text-outline">{t("adr")}</span>
            <textarea
              data-qa="review-edit-adr"
              value={editAdr}
              onChange={(e) => setEditAdr(e.target.value)}
              className="min-h-28 w-full rounded border border-outline-variant bg-surface-container-low p-2 font-body text-body"
            />
          </label>
          <label className="block space-y-1">
            <span className="font-body text-meta uppercase text-outline">{t("tags")}</span>
            <input
              data-qa="review-edit-tags"
              value={editTags}
              onChange={(e) => setEditTags(e.target.value)}
              className="min-h-8 w-full rounded border border-outline-variant bg-surface-container-low px-2 font-body text-body"
            />
          </label>
          <label className="block space-y-1">
            <span className="font-body text-meta uppercase text-outline">{t("outcome")}</span>
            <select
              data-qa="review-edit-outcome"
              value={editOutcome}
              onChange={(e) => setEditOutcome(e.target.value as ExperienceOutcome)}
              className="min-h-8 w-full rounded border border-outline-variant bg-surface-container-low px-2 font-body text-body"
            >
              <option value="success">success</option>
              <option value="partial">partial</option>
              <option value="failure">failure</option>
            </select>
          </label>
          <div className="flex flex-wrap gap-2">
            <button
              type="submit"
              data-qa="review-save"
              disabled={busy}
              className="min-h-8 rounded bg-primary-container px-3 py-1 font-body text-body font-semibold text-on-primary-container"
            >
              {t("save")}
            </button>
            <button
              type="button"
              data-qa="review-edit-cancel"
              disabled={busy}
              onClick={() => setEditing(false)}
              className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body"
            >
              {t("cancel")}
            </button>
          </div>
        </form>
      ) : (
        <>
          <div className="min-h-0 flex-1 space-y-3 overflow-auto p-3">
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="rounded border border-outline-variant px-1.5 font-mono text-meta">
                {selected.project_id}
              </span>
              {selected.reviewed === false && selected.archived_by !== "rejected" ? (
                <span
                  data-qa="unreviewed-badge"
                  className="rounded border border-dashed border-secondary/60 px-1.5 font-body text-meta text-secondary uppercase"
                >
                  {t("unreviewed")}
                </span>
              ) : null}
            </div>
            <h3 className="font-body text-panel font-semibold text-on-surface">
              {selected.title || t("noAdr")}
            </h3>
            <p className="font-body text-body text-on-surface-variant">
              {t("review.suggestedBy", { agent: selected.agent })}
            </p>
            <p className="font-mono text-meta text-outline">
              {selected.outcome} · {formatExperienceTime(selected.created_at)}
            </p>
            {(selected.tags ?? []).length > 0 ? (
              <div className="flex flex-wrap gap-1">
                {selected.tags.map((tag) => (
                  <span
                    key={tag}
                    className="rounded border border-outline-variant/50 px-1 font-mono text-meta"
                  >
                    {tag}
                  </span>
                ))}
              </div>
            ) : null}
            <section className="space-y-1">
              <h4 className="font-body text-meta font-semibold tracking-label uppercase text-outline">
                {t("adr")}
              </h4>
              <pre className="whitespace-pre-wrap break-words rounded border border-outline-variant/40 bg-surface-container-low p-2 font-mono text-body text-on-surface">
                {selected.adr_summary || t("noAdr")}
              </pre>
            </section>
            <p className="font-body text-meta text-on-surface-variant">
              {t("review.suggestedBy", { agent: selected.agent })}
            </p>
            <p className="font-mono text-meta text-outline">
              {selected.project_id} · {selected.outcome} · {formatExperienceTime(selected.created_at)}
            </p>
            <p className="font-body text-meta text-on-surface-variant">{t("useCount")}: {selected.use_count ?? 0}</p>
            <p className="font-body text-meta text-on-surface-variant">
              {t("lastUsed")}:{" "}
              {selected.last_used_at ? formatExperienceTime(selected.last_used_at) : "—"}
            </p>
            {experienceHasHygieneMask(selected) ? (
              <section
                data-qa="review-hygiene-findings"
                className="rounded border border-tertiary/40 bg-surface-container-high/40 p-2"
              >
                <h4 className="font-body text-meta font-semibold uppercase text-tertiary">
                  {t("review.hygieneFindings")}
                </h4>
                <p className="font-body text-meta text-on-surface-variant">
                  {t("review.hygieneFindingsHint")}
                </p>
              </section>
            ) : null}
            {actionError ? (
              <p className="font-body text-body text-error" role="alert">
                {actionError}
              </p>
            ) : null}
          </div>
          <div className="flex shrink-0 flex-wrap gap-2 border-t border-outline-variant px-3 py-2">
            {statId === "rejected" || selected.archived_by === "rejected" ? (
              <button
                type="button"
                data-qa="review-undo"
                disabled={busy}
                onClick={() => void runUndo(selected.id)}
                className="min-h-8 rounded bg-primary-container px-3 py-1 font-body text-body font-semibold text-on-primary-container"
              >
                {t("review.undo")}
              </button>
            ) : (
              <>
                <button
                  type="button"
                  data-qa="review-approve"
                  disabled={busy || selected.reviewed === true}
                  onClick={() => void runApprove([selected.id])}
                  className="min-h-8 rounded bg-primary-container px-3 py-1 font-body text-body font-semibold text-on-primary-container"
                >
                  {t("review.approve")}
                </button>
                <button
                  type="button"
                  data-qa="review-edit"
                  disabled={busy}
                  onClick={beginEdit}
                  className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body"
                >
                  {t("edit")}
                </button>
                <button
                  type="button"
                  data-qa="review-reject"
                  disabled={busy}
                  onClick={() => void runReject([selected.id])}
                  className="min-h-8 rounded border border-error/50 px-3 py-1 font-body text-body text-error"
                >
                  {t("review.reject")}
                </button>
                <button
                  type="button"
                  data-qa="review-open-drawer"
                  disabled={busy}
                  onClick={() => void openExperience(selected.id)}
                  className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-meta text-on-surface-variant"
                >
                  {t("review.openFull")}
                </button>
              </>
            )}
          </div>
        </>
      )}
    </div>
  );

  const inspectorPane = (
    <div className="flex h-full min-h-0 flex-col overflow-auto p-3" data-qa="vault-review-inspector">
      {!selected ? (
        <p className="font-body text-body text-on-surface-variant">{t("review.noSelection")}</p>
      ) : (
        <div className="space-y-3 font-mono text-meta text-on-surface">
          <h3 className="font-body text-panel font-semibold tracking-label uppercase text-outline">
            {t("metadata")}
          </h3>
          <p data-qa="review-inspector-id" className="break-all">
            <span className="font-body text-outline uppercase">{t("metadata")}: </span>
            {selected.id}
          </p>
          <p>
            <span className="font-body text-outline uppercase">{t("useCount")}: </span>
            {selected.use_count ?? 0}
          </p>
          <p>
            <span className="font-body text-outline uppercase">{t("lastUsed")}: </span>
            {selected.last_used_at ? formatExperienceTime(selected.last_used_at) : "—"}
          </p>
          <p>
            <span className="font-body text-outline uppercase">{t("updated")}: </span>
            {selected.updated_at ? formatExperienceTime(selected.updated_at) : "—"}
          </p>
          <p>
            <span className="font-body text-outline uppercase">{t("source")}: </span>
            {selected.source || "—"}
          </p>
          <p>
            <span className="font-body text-outline uppercase">{t("relatedTask")}: </span>
            {selected.related_task_id || "—"}
          </p>
          <p>
            <span className="font-body text-outline uppercase">{t("review.statusLabel")} </span>
            {selected.status ?? "active"}
            {selected.archived_by ? ` · ${selected.archived_by}` : ""}
          </p>
          <section className="space-y-1" data-qa="review-inspector-masked-content">
            <h4 className="font-body text-meta font-semibold tracking-label uppercase text-outline">
              {t("review.maskedContent")}
            </h4>
            <pre className="whitespace-pre-wrap break-words rounded border border-outline-variant/40 bg-surface-container-low p-2 font-mono text-body">
              {maskedReviewContent(selected) || "—"}
            </pre>
          </section>
          <p className="font-body text-meta text-on-surface-variant">
            {t("review.suggestedBy", { agent: selected.agent })}
          </p>
          <p className="font-mono text-meta text-outline">{selected.project_id}</p>
          <p className="font-mono text-meta text-outline">{selected.outcome}</p>
          <p className="font-body text-meta text-on-surface-variant">
            {t("title")}: {selected.title || "—"}
          </p>
          <p className="font-body text-meta text-on-surface-variant">
            {t("tags")}: {(selected.tags ?? []).join(", ") || "—"}
          </p>
          <p className="font-body text-meta text-on-surface-variant">
            {t("created")}: {formatExperienceTime(selected.created_at)}
          </p>
        </div>
      )}
    </div>
  );

  return (
    <section
      data-qa-vault-review={state.phase}
      className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden"
    >
      {/* Same PanelGrid L2 occupancy pattern as AgentsHealthPanel. */}
      <PanelGrid
        className="min-h-[28rem] flex-1"
        list={listPane}
        detail={detailPane}
        inspector={inspectorPane}
      />

      {confirm ? (
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center bg-surface-container-lowest/80 px-4 backdrop-blur-sm"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby={confirmTitleId}
          data-qa="vault-review-confirm"
        >
          <div
            ref={confirmRef}
            className="w-full max-w-md border border-outline-variant bg-surface-container-high p-5 shadow-lg"
          >
            <h2 id={confirmTitleId} className="font-body text-base font-semibold text-on-surface">
              {confirm === "approve"
                ? t("review.confirmApproveTitle")
                : t("review.confirmRejectTitle")}
            </h2>
            <p className="mt-2 font-body text-body text-on-surface-variant">
              {confirm === "approve"
                ? t("review.confirmApproveBody", {
                    count: checkedIds.length > 0 ? checkedIds.length : 1,
                  })
                : t("review.confirmRejectBody", {
                    count: checkedIds.length > 0 ? checkedIds.length : 1,
                  })}
            </p>
            <div className="mt-5 flex flex-wrap justify-end gap-2">
              <button
                type="button"
                data-qa="review-confirm-cancel"
                disabled={busy}
                onClick={() => setConfirm(null)}
                className="min-h-8 rounded border border-outline-variant px-3 py-1 font-body text-body"
              >
                {t("cancel")}
              </button>
              <button
                type="button"
                data-qa="review-confirm-ok"
                disabled={busy}
                onClick={() => {
                  const ids =
                    checkedIds.length > 0
                      ? checkedIds
                      : selected
                        ? [selected.id]
                        : [];
                  if (confirm === "approve") void runApprove(ids);
                  else void runReject(ids);
                }}
                className={`min-h-8 rounded px-3 py-1 font-body text-body font-semibold ${
                  confirm === "reject"
                    ? "bg-error-container text-on-error-container"
                    : "bg-primary-container text-on-primary-container"
                }`}
              >
                {confirm === "approve" ? t("review.approve") : t("review.reject")}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}
