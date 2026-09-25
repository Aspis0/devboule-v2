// @vitest-environment happy-dom
// The looked-at record and the OS toast, through the surface that owns the
// selection. A workspace row click moves the record WITH the click, because the
// toast gate judges a roster push against it — so the write belongs to the store
// update that decides the selection, not to a render or a paint-time effect.
// Nothing here calls `reportSelection`: the bridge the surface installs is the
// only writer, exactly as in the app.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The notification plugin records what goes out; the window's own answers come
// through the daemon pipe instead (see `answerWindow` below), because that is
// how the production read reaches the OS: `getCurrentWindow()` asks
// `__TAURI_INTERNALS__` for the window label and the three questions are
// `invoke` calls. A test has no window to look at and no toast service to read.
vi.mock("@tauri-apps/plugin-notification", () => ({
  sendNotification: vi.fn(),
}));

import { sendNotification } from "@tauri-apps/plugin-notification";
import {
  agentSession,
  afterEachHarness,
  beforeEachHarness,
  pushSnapshots,
  renderWorkspace,
  unmountWorkspace,
  workspace,
} from "./bulkCloseHarness";
import { invoke } from "@tauri-apps/api/core";
import { createSessionStateChannel, sessionsList, workspacesList } from "../../lib/tauri";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import { lookedAtSessionId } from "./presence";
import { forgetAttentionFor } from "./attentionNotice";
import { resetSharedSessionControllerForTests } from "./workspaceSessions";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const secondWorkspace = { ...workspace, id: "workspace-2", title: "second" };

const inWorkspace = (id: string, title: string, workspaceId: string): Session => ({
  ...agentSession(id, title),
  workspaceId,
});

const rosterOf = (
  members: Array<{ id: string; workspace: string; raisedAt?: number }>,
): SessionStateSnapshot[] =>
  members.map(({ id, workspace: workspaceId, raisedAt }) => ({
    id,
    workspaceId,
    kind: "acp" as const,
    title: id,
    state: { type: "live" as const, generation: 1 },
    elapsedMs: 0,
    ...(raisedAt === undefined
      ? {}
      : { attention: { reason: "finished" as const, atMs: raisedAt } }),
  }));

/**
 * Let the toast path finish. It awaits the window answer, the notification
 * plugin's module, the permission command and the sender, and the plugin's own
 * hop is not a microtask — under a full-suite load it needs more than one turn
 * of the (fake) clock, so this bounded pump is what makes "no toast" and "the
 * toast has landed" the same answer here and in the file run alone.
 */
async function settleToastPath(): Promise<void> {
  for (let turn = 0; turn < 40; turn += 1) await vi.advanceTimersByTimeAsync(1);
}

/** The daemon's push, called as the channel calls it: no act, no React flush. */
function pushRoster(snapshots: SessionStateSnapshot[]): void {
  const listener = vi.mocked(createSessionStateChannel).mock.calls[0]?.[0];
  if (listener === undefined) throw new Error("roster watch is not wired");
  listener(snapshots);
}

/**
 * The toasts sent from `mark` on. Reading the sender — not a count of
 * notifications — is what makes "held back" and "never got as far as sending"
 * different answers, and slicing at `mark` keeps a hop that lands after another
 * test finished out of this one's claim.
 */
function titlesSince(mark: number): string[] {
  return vi
    .mocked(sendNotification)
    .mock.calls.slice(mark)
    .map(([options]) => (options as { title: string }).title);
}

function sentToasts(): number {
  return vi.mocked(sendNotification).mock.calls.length;
}

function workspaceRow(title: string): HTMLButtonElement {
  const row = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
    (candidate) => candidate.textContent?.includes(title),
  );
  if (row === undefined) throw new Error(`workspace row did not render: ${title}`);
  return row;
}

const bothAgents = rosterOf([
  { id: "agent-one", workspace: "workspace-1" },
  { id: "agent-two", workspace: "workspace-2" },
]);

/** The window's own state, answered where the production read asks it: the OS
 *  truth about this window arrives as three daemon calls, so the pipe is the
 *  seam — and "seen and focused" is the state the gate must not look past. */
function answerWindow(command: unknown): unknown {
  if (
    command === "plugin:window|is_visible" ||
    command === "plugin:window|is_focused" ||
    command === "plugin:notification|is_permission_granted"
  ) {
    return true;
  }
  if (command === "plugin:window|is_minimized") return false;
  return undefined;
}

type TauriInternals = typeof globalThis & {
  __TAURI_INTERNALS__?: {
    metadata: { currentWindow: { label: string } };
    invoke: (command: string) => Promise<unknown>;
  };
};

beforeEach(() => {
  beforeEachHarness();
  // The window module is the Tauri API's own code: it asks `__TAURI_INTERNALS__`
  // for this window's label and sends its three questions down the same pipe as
  // every other command. Answering there is what makes the window read real
  // rather than unreachable — the mock of `sendNotification` below stays the
  // only place a toast is observed.
  const internals: TauriInternals = globalThis;
  internals.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" } },
    invoke: async (command: string) => answerWindow(command),
  };
  vi.mocked(invoke).mockImplementation(async (command: unknown) => answerWindow(command));
  resetSharedSessionControllerForTests();
  // The notifier's records are module state, like the roster store's: each test
  // starts with an app that has announced nothing.
  forgetAttentionFor(new Set());
  vi.mocked(workspacesList).mockResolvedValue([workspace, secondWorkspace]);
  vi.mocked(sessionsList).mockResolvedValue([
    inWorkspace("agent-one", "Agent one", "workspace-1"),
    inWorkspace("agent-two", "Agent two", "workspace-2"),
  ]);
});

afterEach(async () => {
  // Every publication this test started has to finish before the next one
  // inherits a withdrawn record: a suppressed raise is deliberately left due.
  await settleToastPath();
  await afterEachHarness();
});

describe("the toast gate, judged by the surface's own write", () => {
  it("holds the new workspace's raise back in the task the switch happened in", async () => {
    await renderWorkspace();
    await pushSnapshots(bothAgents);
    expect(lookedAtSessionId()).toBe("agent-one");

    // Deliberately not wrapped in act: this is the case the review named. The
    // click decides the selection inside its own handler; the record has to move
    // in that same step, because the roster speaks in a task after it and the
    // gate judges that push against the record.
    const acting = globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };
    acting.IS_REACT_ACT_ENVIRONMENT = false;
    try {
      workspaceRow("second").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    } finally {
      acting.IS_REACT_ACT_ENVIRONMENT = true;
    }
    expect(lookedAtSessionId()).toBe("agent-two");

    // One roster, both raises: the session now on screen, and a session this
    // window shows nowhere. Only the second is news the user has not seen.
    const before = sentToasts();
    pushRoster(
      rosterOf([
        { id: "agent-one", workspace: "workspace-1", raisedAt: 3_000 },
        { id: "agent-two", workspace: "workspace-2", raisedAt: 2_000 },
      ]),
    );
    await settleToastPath();
    expect(titlesSince(before)).toEqual(["agent-one — finished"]);
  });

  it("withdraws the record with the surface, so nothing is held back for a session nobody shows", async () => {
    await renderWorkspace();
    await pushSnapshots(bothAgents);
    expect(lookedAtSessionId()).toBe("agent-one");

    // Settings taking the screen unmounts the surface, and the withdrawal is
    // part of that step: by the time the tree is gone the record already says
    // that no session is on screen.
    await unmountWorkspace();
    expect(lookedAtSessionId()).toBeNull();

    // The roster keeps speaking after the surface is gone — the watch is
    // app-scope — and the raise for the session the user last saw is due.
    const before = sentToasts();
    pushRoster(rosterOf([{ id: "agent-one", workspace: "workspace-1", raisedAt: 4_000 }]));
    await settleToastPath();
    expect(titlesSince(before)).toEqual(["agent-one — finished"]);
  });
});
