"use client";

import { useRef, useState, type FormEvent } from "react";
import { useLounge } from "@/components/lounge-provider";
import { formatExperienceTime, type ExperienceOutcome, type LoungeExperience } from "@/lib/lounge";
import { formatDisplayPath } from "@/lib/experience";
import { vaultStrings as s } from "@/lib/strings/vault";

type ExperienceDrawerProps = {
  open: boolean;
  experience: LoungeExperience | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
};

export function ExperienceDrawer({
  open,
  experience,
  loading,
  error,
  onClose,
}: ExperienceDrawerProps) {
  const {
    updateExperience,
    archiveExperience,
    unarchiveExperience,
    pinExperience,
    markExperienceReviewed,
  } = useLounge();
  const asideRef = useRef<HTMLElement | null>(null);
  const [adr, setAdr] = useState("");
  const [projectId, setProjectId] = useState("");
  const [outcome, setOutcome] = useState<ExperienceOutcome>("success");
  const [tags, setTags] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  if (!open) {
    return null;
  }

  const archived = experience?.status === "archived";
  const pinned = Boolean(experience?.is_pinned);

  async function runAction(action: () => Promise<void>) {
    setBusy(true);
    setActionError(null);
    try {
      await action();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-scrim/40"
      role="presentation"
      onClick={onClose}
    >
      <aside
        ref={asideRef}
        data-qa="experience-drawer"
        data-mode="edit"
        role="dialog"
        aria-modal="true"
        aria-label="Experience detail"
        className="flex h-full w-full max-w-xl flex-col border-l border-outline-variant bg-surface-container shadow-xl"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="flex shrink-0 items-start justify-between gap-3 border-b border-outline-variant px-4 py-3">
          <div className="min-w-0">
            <div className="font-body text-meta font-semibold tracking-label text-outline uppercase">
              {s.metadata}
            </div>
            <h2 className="truncate font-body text-panel font-semibold text-on-surface">
              {experience?.project_id ?? s.loading}
            </h2>
            {experience ? (
              <div className="mt-1 flex flex-wrap items-center gap-1.5 font-mono text-meta text-on-surface-variant">
                <span>{formatExperienceTime(experience.created_at)}</span>
                {archived ? (
                  <span className="rounded border border-outline-variant px-1 text-outline uppercase">
                    {s.archivedLabel}
                  </span>
                ) : null}
                {experience.reviewed === false ? (
                  <span className="rounded border border-secondary/40 px-1 text-secondary uppercase">
                    {s.unreviewed}
                  </span>
                ) : null}
                {pinned ? (
                  <span className="rounded border border-primary/40 px-1 text-primary uppercase">
                    pinned
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
          <button
            type="button"
            onClick={onClose}
            className="min-h-8 rounded border border-outline-variant px-2 py-1 font-body text-meta text-on-surface-variant hover:bg-surface-container-high"
            aria-label="Close"
          >
            ✕
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-auto px-4 py-3 font-body text-body">
          {loading ? (
            <p className="text-on-surface-variant">{s.loading}</p>
          ) : error ? (
            <p className="text-error">{error || s.loadError}</p>
          ) : experience ? (
            <div className="space-y-4" data-qa="experience-view-mode">
              <section className="space-y-2">
                <h3 className="font-mono text-meta font-semibold uppercase text-outline">{s.adr}</h3>
                <div className="whitespace-pre-wrap break-words rounded border border-outline-variant/50 bg-surface-container-high/50 px-3 py-2 leading-relaxed text-on-surface">
                  {experience.adr_summary}
                </div>
              </section>
              {experience.original_content &&
              experience.original_content !== experience.adr_summary ? (
                <section className="space-y-2">
                  <h3 className="font-mono text-meta font-semibold uppercase text-outline">
                    {s.originalContent}
                  </h3>
                  <div className="whitespace-pre-wrap break-words rounded border border-outline-variant/40 bg-surface-container-high/30 px-3 py-2 text-meta text-on-surface-variant">
                    {experience.original_content}
                  </div>
                </section>
              ) : null}
              <dl className="grid grid-cols-2 gap-2 font-mono text-meta text-on-surface-variant">
                <div>
                  <dt className="uppercase text-outline">{s.agent}</dt>
                  <dd className="text-on-surface">{experience.agent}</dd>
                </div>
                <div>
                  <dt className="uppercase text-outline">{s.source}</dt>
                  <dd className="text-on-surface">{experience.type}</dd>
                </div>
                <div>
                  <dt className="uppercase text-outline">{s.useCount}</dt>
                  <dd className="text-on-surface">{experience.use_count ?? 0}</dd>
                </div>
                <div>
                  <dt className="uppercase text-outline">{s.lastUsed}</dt>
                  <dd className="text-on-surface">
                    {experience.last_used_at
                      ? formatExperienceTime(experience.last_used_at)
                      : "—"}
                  </dd>
                </div>
                <div>
                  <dt className="uppercase text-outline">{s.updated}</dt>
                  <dd className="text-on-surface">
                    {experience.updated_at
                      ? formatExperienceTime(experience.updated_at)
                      : "—"}
                  </dd>
                </div>
                <div>
                  <dt className="uppercase text-outline">{s.relatedTask}</dt>
                  <dd className="truncate text-on-surface">
                    {experience.related_task_id ?? "—"}
                  </dd>
                </div>
              </dl>
              {(experience.tags ?? []).length > 0 ? (
                <div className="flex flex-wrap gap-1">
                  {experience.tags.map((tag) => (
                    <span
                      key={tag}
                      className="rounded border border-outline-variant/60 px-1.5 py-0.5 font-mono text-meta text-outline"
                    >
                      {tag}
                    </span>
                  ))}
                </div>
              ) : null}

              <form
                id="experience-edit-form"
                data-qa="experience-edit-mode"
                className="space-y-3 border-t border-outline-variant/50 pt-3"
                onSubmit={(event: FormEvent) => {
                  event.preventDefault();
                  void runAction(async () => {
                    await updateExperience(experience.id, {
                      adr_summary: adr || experience.adr_summary,
                      project_id: projectId || experience.project_id,
                      outcome,
                      tags: (tags || (experience.tags ?? []).join(", "))
                        .split(",")
                        .map((tag) => tag.trim())
                        .filter(Boolean),
                    });
                  });
                }}
              >
                <label className="block space-y-1">
                  <span className="font-mono text-meta uppercase text-outline">{s.project}</span>
                  <input
                    defaultValue={experience.project_id}
                    onChange={(event) => setProjectId(event.target.value)}
                    className="w-full rounded border border-outline-variant bg-surface-container-high px-2 py-1.5 font-mono text-body"
                  />
                </label>
                <label className="block space-y-1">
                  <span className="font-mono text-meta uppercase text-outline">{s.outcome}</span>
                  <select
                    defaultValue={experience.outcome}
                    onChange={(event) => setOutcome(event.target.value as ExperienceOutcome)}
                    className="w-full rounded border border-outline-variant bg-surface-container-high px-2 py-1.5 font-body text-body"
                  >
                    <option value="success">success</option>
                    <option value="partial">partial</option>
                    <option value="failure">failure</option>
                  </select>
                </label>
                <label className="block space-y-1">
                  <span className="font-mono text-meta uppercase text-outline">{s.tags}</span>
                  <input
                    defaultValue={(experience.tags ?? []).join(", ")}
                    onChange={(event) => setTags(event.target.value)}
                    className="w-full rounded border border-outline-variant bg-surface-container-high px-2 py-1.5 font-mono text-body"
                  />
                </label>
                <label className="block space-y-1">
                  <span className="font-mono text-meta uppercase text-outline">{s.adr}</span>
                  <textarea
                    data-qa="experience-adr-input"
                    defaultValue={experience.adr_summary}
                    onChange={(event) => setAdr(event.target.value)}
                    rows={8}
                    className="w-full rounded border border-outline-variant bg-surface-container-high px-2 py-1.5 font-body text-body leading-relaxed"
                  />
                </label>
              </form>
              {actionError ? <p className="text-error">{actionError}</p> : null}
            </div>
          ) : null}
        </div>

        {experience ? (
          <footer className="flex shrink-0 flex-wrap gap-2 border-t border-outline-variant px-4 py-3">
            <button
              type="button"
              disabled={busy}
              data-qa="experience-edit-button"
              onClick={() => {
                setAdr(experience.adr_summary);
                setProjectId(experience.project_id);
                setOutcome(experience.outcome);
                setTags((experience.tags ?? []).join(", "));
                document
                  .querySelector<HTMLTextAreaElement>('[data-qa="experience-adr-input"]')
                  ?.focus();
              }}
              className="min-h-9 rounded border border-outline-variant px-3 py-1.5 font-body text-meta hover:bg-surface-container-high"
            >
              {s.edit}
            </button>
            <button
              type="submit"
              form="experience-edit-form"
              disabled={busy}
              className="min-h-9 rounded border border-primary bg-primary px-3 py-1.5 font-body text-meta text-on-primary hover:opacity-90 disabled:opacity-60"
            >
              {s.save}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void runAction(async () => {
                  if (archived) {
                    await unarchiveExperience(experience.id);
                  } else if (window.confirm(s.archiveConfirm)) {
                    await archiveExperience(experience.id);
                    onClose();
                  }
                })
              }
              className="min-h-9 rounded border border-outline-variant px-3 py-1.5 font-body text-meta hover:bg-surface-container-high"
            >
              {archived ? s.unarchive : s.archive}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void runAction(async () => pinExperience(experience.id, !pinned))
              }
              className="min-h-9 rounded border border-outline-variant px-3 py-1.5 font-body text-meta hover:bg-surface-container-high"
            >
              {pinned ? s.unpin : s.pin}
            </button>
            {experience.reviewed === false ? (
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void runAction(async () => markExperienceReviewed(experience.id))
                }
                className="min-h-9 rounded border border-secondary/50 px-3 py-1.5 font-body text-meta text-secondary hover:bg-secondary/10"
              >
                {s.markReviewed}
              </button>
            ) : null}
            {archived && experience.archived_at ? (
              <span className="self-center font-mono text-meta text-outline">
                {s.restoreTtl} · {formatExperienceTime(experience.archived_at)}
              </span>
            ) : null}
          </footer>
        ) : null}
      </aside>
    </div>
  );
}

/** PATH-01: visible cross-platform path samples from indexed workspace rows. */
export function VaultPathReference({ paths }: { paths: string[] }) {
  const visible = paths.filter((path) => path.trim().length > 0);
  if (visible.length === 0) {
    return null;
  }
  return (
    <div
      data-qa="path-reference"
      className="shrink-0 border-t border-outline-variant/50 px-2.5 py-1.5 font-mono text-meta text-on-surface-variant"
    >
      <div className="mb-1 font-semibold uppercase tracking-label text-outline">{s.pathReference}</div>
      <div className="space-y-0.5 break-all">
        {visible.map((path) => (
          <div key={path}>{formatDisplayPath(path)}</div>
        ))}
      </div>
    </div>
  );
}
