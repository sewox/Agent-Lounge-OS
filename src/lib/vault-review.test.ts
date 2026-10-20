import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  applyReviewFilters,
  assertCounterConsistency,
  experienceHasHygieneMask,
  maskedReviewContent,
  redactHomePaths,
  REVIEW_SCORE_BAND,
  sanitizeReviewRow,
  sortReviewQueue,
  sortSearchByReviewedBand,
  type ReviewableExperience,
} from "./vault-review.ts";

function row(partial: Partial<ReviewableExperience> & Pick<ReviewableExperience, "id">): ReviewableExperience {
  return {
    agent: "cursor",
    project_id: "Agent-Lounge-OS",
    adr_summary: "plain",
    outcome: "success",
    tags: [],
    created_at: "2026-10-20T12:00:00.000Z",
    status: "active",
    reviewed: false,
    ...partial,
  };
}

describe("vault-review ordering", () => {
  it("sorts hygiene-flagged rows before plain unreviewed", () => {
    const plain = row({
      id: "plain",
      created_at: "2026-10-20T18:00:00.000Z",
      adr_summary: "clean text",
    });
    const hygiene = row({
      id: "hygiene",
      created_at: "2026-10-19T10:00:00.000Z",
      adr_summary: "path <home>/secret",
    });
    const sorted = sortReviewQueue([plain, hygiene]);
    assert.equal(sorted[0]?.id, "hygiene");
    assert.equal(sorted[1]?.id, "plain");
    assert.equal(experienceHasHygieneMask(hygiene), true);
    assert.equal(experienceHasHygieneMask(plain), false);
  });

  it("keeps newer first within the same hygiene band", () => {
    const older = row({ id: "older", created_at: "2026-10-01T00:00:00.000Z" });
    const newer = row({ id: "newer", created_at: "2026-10-20T00:00:00.000Z" });
    const sorted = sortReviewQueue([older, newer]);
    assert.equal(sorted[0]?.id, "newer");
  });
});

describe("vault-review counter consistency", () => {
  it("pending matches canonical unreviewed count", () => {
    assert.equal(assertCounterConsistency(3, { pending: 3 }), true);
    assert.equal(assertCounterConsistency(2, { pending: 3 }), false);
  });
});

describe("vault-review filters", () => {
  it("filters by project, agent, outcome, and hygiene", () => {
    const rows = [
      row({
        id: "a",
        project_id: "A",
        agent: "cursor",
        outcome: "success",
        adr_summary: "<redacted-token>",
      }),
      row({
        id: "b",
        project_id: "B",
        agent: "grok-bot",
        outcome: "failure",
        adr_summary: "clean",
      }),
    ];
    const filtered = applyReviewFilters(rows, {
      projectId: "A",
      agent: "cursor",
      outcome: "success",
      hygieneOnly: true,
    });
    assert.deepEqual(
      filtered.map((r) => r.id),
      ["a"],
    );
  });
});

describe("vault-review search band ordering", () => {
  it("ranks reviewed above unreviewed within REVIEW_SCORE_BAND", () => {
    const approved = row({
      id: "ok",
      reviewed: true,
      created_at: "2026-01-01T00:00:00.000Z",
      score: 0.8,
    } as ReviewableExperience & { score: number });
    const pending = row({
      id: "wait",
      reviewed: false,
      created_at: "2026-06-01T00:00:00.000Z",
      score: 0.8 + REVIEW_SCORE_BAND / 2,
    } as ReviewableExperience & { score: number });
    const sorted = sortSearchByReviewedBand([pending, approved]);
    assert.equal(sorted[0]?.id, "ok");
    assert.equal(sorted[1]?.id, "wait");
  });

  it("keeps strongly relevant unreviewed above weakly relevant approved", () => {
    const strongPending = row({
      id: "strong",
      reviewed: false,
      score: 0.95,
    } as ReviewableExperience & { score: number });
    const weakApproved = row({
      id: "weak",
      reviewed: true,
      score: 0.3,
    } as ReviewableExperience & { score: number });
    const sorted = sortSearchByReviewedBand([weakApproved, strongPending]);
    assert.equal(sorted[0]?.id, "strong");
    assert.equal(sorted[1]?.id, "weak");
  });
});

describe("vault-review masking", () => {
  it("redacts home roots and sanitizes rows for the DOM", () => {
    assert.equal(redactHomePaths("/Users/ci/projects/secret"), "<home>/projects/secret");
    assert.equal(redactHomePaths("/home/sercan/dev"), "<home>/dev");
    assert.equal(redactHomePaths("C:\\Users\\sercan\\dev"), "<home>\\dev");
    assert.equal(redactHomePaths("C:/Users/sercan/dev"), "<home>/dev");
    assert.equal(redactHomePaths("/root"), "<home>");
    assert.equal(redactHomePaths("/root/.config"), "<home>/.config");
    assert.equal(redactHomePaths("/var/home/fedora/work"), "<home>/work");
    const dirty = row({
      id: "x",
      adr_summary: "see /Users/ci/leak and /var/home/u/x",
      original_content: "/Users/ci/raw",
    });
    const clean = sanitizeReviewRow(dirty);
    assert.equal(clean.original_content, null);
    assert.equal(clean.adr_summary.includes("/Users/"), false);
    assert.equal(clean.adr_summary.includes("/var/home/"), false);
    assert.equal(maskedReviewContent(clean).includes("/Users/"), false);
  });
});
