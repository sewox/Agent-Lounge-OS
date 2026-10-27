/** Deterministic event_journal fixtures for Journal-UI-1 E2E (JU1–JU4). */

export type JournalEntryFixture = {
  seq: number;
  eventId: string;
  timestamp: string;
  actor: string;
  actionType: string;
  projectId: string | null;
  taskId: string | null;
  payloadJson: string;
  prevHash: string;
  entryHash: string;
};

export type JournalVerificationFixture = {
  valid: boolean;
  checkedCount: number;
  headHash: string | null;
  brokenAtSeq: number | null;
  brokenEventId: string | null;
  reason: string | null;
};

const BASE_MS = Date.parse("2026-01-01T00:00:00.000Z");

function at(offsetMs: number): string {
  return new Date(BASE_MS + offsetMs).toISOString();
}

function hash(n: number): string {
  return `a${String(n).padStart(63, "0")}`;
}

const TASK_ID = "task-a2a-demo";
const PROJECT_ID = "Agent-Lounge-OS";

/** Multi-step A2A lifecycle + experience + control — chainValid metrics: 6000 / 3500 / 2500. */
export const FULL_JOURNAL_ENTRIES: JournalEntryFixture[] = [
  {
    seq: 1,
    eventId: "jev-task-requested",
    timestamp: at(0),
    actor: "mcp",
    actionType: "task.requested",
    projectId: PROJECT_ID,
    taskId: TASK_ID,
    payloadJson: "{}",
    prevHash: "GENESIS",
    entryHash: hash(1),
  },
  {
    seq: 2,
    eventId: "jev-task-claimed",
    timestamp: at(500),
    actor: "kernel",
    actionType: "task.claimed",
    projectId: PROJECT_ID,
    taskId: TASK_ID,
    payloadJson: "{}",
    prevHash: hash(1),
    entryHash: hash(2),
  },
  {
    seq: 3,
    eventId: "jev-task-approval-pending",
    timestamp: at(1000),
    actor: "kernel",
    actionType: "task.approval_pending",
    projectId: PROJECT_ID,
    taskId: TASK_ID,
    payloadJson: "{}",
    prevHash: hash(2),
    entryHash: hash(3),
  },
  {
    seq: 4,
    eventId: "jev-task-approval-approved",
    timestamp: at(4500),
    actor: "ui",
    actionType: "task.approval_approved",
    projectId: PROJECT_ID,
    taskId: TASK_ID,
    payloadJson: "{}",
    prevHash: hash(3),
    entryHash: hash(4),
  },
  {
    seq: 5,
    eventId: "jev-task-completed",
    timestamp: at(6000),
    actor: "agent",
    actionType: "task.completed",
    projectId: PROJECT_ID,
    taskId: TASK_ID,
    payloadJson: "{}",
    prevHash: hash(4),
    entryHash: hash(5),
  },
  {
    seq: 6,
    eventId: "jev-exp-recorded",
    timestamp: at(7000),
    actor: "kernel",
    actionType: "experience.recorded",
    projectId: PROJECT_ID,
    taskId: TASK_ID,
    payloadJson: "{}",
    prevHash: hash(5),
    entryHash: hash(6),
  },
  {
    seq: 7,
    eventId: "jev-exp-reviewed",
    timestamp: at(8000),
    actor: "ui",
    actionType: "experience.reviewed",
    projectId: PROJECT_ID,
    taskId: null,
    payloadJson: "{}",
    prevHash: hash(6),
    entryHash: hash(7),
  },
  {
    seq: 8,
    eventId: "jev-control-halted",
    timestamp: at(9000),
    actor: "ui",
    actionType: "control.halted",
    projectId: null,
    taskId: null,
    payloadJson: "{}",
    prevHash: hash(7),
    entryHash: hash(8),
  },
  {
    seq: 9,
    eventId: "jev-control-resumed",
    timestamp: at(10000),
    actor: "ui",
    actionType: "control.resumed",
    projectId: null,
    taskId: null,
    payloadJson: "{}",
    prevHash: hash(8),
    entryHash: hash(9),
  },
];

export const FULL_JOURNAL_VERIFICATION: JournalVerificationFixture = {
  valid: true,
  checkedCount: FULL_JOURNAL_ENTRIES.length,
  headHash: hash(9),
  brokenAtSeq: null,
  brokenEventId: null,
  reason: null,
};

export const BROKEN_JOURNAL_VERIFICATION: JournalVerificationFixture = {
  valid: false,
  checkedCount: 2,
  headHash: hash(1),
  brokenAtSeq: 2,
  brokenEventId: "jev-task-claimed",
  reason: "entry_hash mismatch (payload or fields tampered)",
};

export const EMPTY_JOURNAL_ENTRIES: JournalEntryFixture[] = [];

export const EMPTY_JOURNAL_VERIFICATION: JournalVerificationFixture = {
  valid: true,
  checkedCount: 0,
  headHash: null,
  brokenAtSeq: null,
  brokenEventId: null,
  reason: null,
};

export const JOURNAL_DEMO_TASK_ID = TASK_ID;
