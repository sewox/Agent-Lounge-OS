import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  applyReviewFilters,
  assertCounterConsistency,
  experienceHasHygieneMask,
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
  it("ranks reviewed above unreviewed without dropping either", () => {
    const approved = row({
      id: "ok",
      reviewed: true,
      created_at: "2026-01-01T00:00:00.000Z",
    });
    const pending = row({
      id: "wait",
      reviewed: false,
      created_at: "2026-06-01T00:00:00.000Z",
    });
    const sorted = sortSearchByReviewedBand([pending, approved]);
    assert.equal(sorted[0]?.id, "ok");
    assert.equal(sorted[1]?.id, "wait");
  });
});
