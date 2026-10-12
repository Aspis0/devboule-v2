import { describe, expect, it } from "vitest";
import type { Attention, AttentionReason, Session } from "../../types/ipc";
import { hasBusyChild, shouldToast, type ToastCandidate } from "./attentionToastPolicy";

/** One row of the filter: the raise, who it is about, and whether a toast goes out. */
interface Row {
  reason: AttentionReason;
  /** `true` when the window is not showing this session to the person (unfocused or elsewhere). */
  away: boolean;
  topLevel: boolean;
  childBusy: boolean;
  /** A card the person must answer in every mode. */
  always?: boolean;
  toast: boolean;
}

const ALWAYS_TABLE: readonly Row[] = [
  // A command for another machine or a saved login reaches the person from any depth.
  {
    reason: "permission",
    away: true,
    topLevel: false,
    childBusy: false,
    always: true,
    toast: true,
  },
  { reason: "permission", away: true, topLevel: true, childBusy: false, always: true, toast: true },
  {
    reason: "permission",
    away: false,
    topLevel: false,
    childBusy: false,
    always: true,
    toast: false,
  },
];

const TABLE: readonly Row[] = [
  // A card: the person answers only the cards of a session they started.
  { reason: "permission", away: true, topLevel: true, childBusy: false, toast: true },
  { reason: "permission", away: true, topLevel: false, childBusy: false, toast: false },
  { reason: "permission", away: false, topLevel: true, childBusy: false, toast: false },
  { reason: "permission", away: false, topLevel: false, childBusy: false, toast: false },
  // A finish: a top-level agent that is idle, with no busy child.
  { reason: "finished", away: true, topLevel: true, childBusy: false, toast: true },
  { reason: "finished", away: true, topLevel: true, childBusy: true, toast: false },
  { reason: "finished", away: true, topLevel: false, childBusy: false, toast: false },
  { reason: "finished", away: true, topLevel: false, childBusy: true, toast: false },
  { reason: "finished", away: false, topLevel: true, childBusy: false, toast: false },
  { reason: "finished", away: false, topLevel: true, childBusy: true, toast: false },
  { reason: "finished", away: false, topLevel: false, childBusy: false, toast: false },
  { reason: "finished", away: false, topLevel: false, childBusy: true, toast: false },
  // An error: a top-level agent that failed; children's errors stay in their transcripts.
  { reason: "error", away: true, topLevel: true, childBusy: false, toast: true },
  { reason: "error", away: true, topLevel: true, childBusy: true, toast: true },
  { reason: "error", away: true, topLevel: false, childBusy: false, toast: false },
  { reason: "error", away: false, topLevel: true, childBusy: false, toast: false },
  { reason: "error", away: false, topLevel: false, childBusy: false, toast: false },
];

describe("the toast filter", () => {
  it.each([...TABLE, ...ALWAYS_TABLE])(
    "$reason, away=$away, topLevel=$topLevel, childBusy=$childBusy → toast=$toast",
    (row) => {
      const candidate: ToastCandidate = {
        reason: row.reason,
        away: row.away,
        topLevel: row.topLevel,
        childBusy: row.childBusy,
        always: row.always ?? false,
      };
      expect(shouldToast(candidate)).toBe(row.toast);
    },
  );
});

function session(id: string, patch: Partial<Session> = {}): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "claude",
    title: id,
    state: { type: "live", generation: 1 },
    elapsedMs: null,
    ...patch,
  } as Session;
}

const PERMISSION: Attention = { reason: "permission", atMs: 1 };

describe("hasBusyChild", () => {
  it("is busy while a child works or waits on a card", () => {
    const roster = [
      session("parent"),
      session("child-a", { createdBy: "parent", activity: "working" }),
    ];
    expect(hasBusyChild("parent", roster)).toBe(true);
    const waiting = [
      session("parent"),
      session("child-b", { createdBy: "parent", activity: "blocked" }),
    ];
    expect(hasBusyChild("parent", waiting)).toBe(true);
    const card = [
      session("parent"),
      session("child-c", { createdBy: "parent", activity: "idle", attention: PERMISSION }),
    ];
    expect(hasBusyChild("parent", card)).toBe(true);
  });

  it("is idle when every child is idle, and when it has no children", () => {
    const roster = [
      session("parent"),
      session("child-d", { createdBy: "parent", activity: "idle" }),
      session("other", { createdBy: "someone-else", activity: "working" }),
    ];
    expect(hasBusyChild("parent", roster)).toBe(false);
    expect(hasBusyChild("parent", [session("parent")])).toBe(false);
  });
});
