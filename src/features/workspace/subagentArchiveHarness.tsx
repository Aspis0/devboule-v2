// What the subagent archive's tests share: the surface they render, the
// roster rows they feed it, and the steps a person takes through the menu and
// its ask. Each test file keeps its own mount and its own `vi.mock`.
import { act, type ReactElement } from "react";
import { channelHarness } from "./sessionChannelHarness";
import { AgentChatSurface } from "./AgentChatSurface";
import type { AgentActivityState, Session, SessionState } from "../../types/ipc";

function rosterRow(
  id: string,
  state: SessionState,
  title: string,
  activity?: AgentActivityState,
): Session {
  return {
    id,
    workspaceId: null,
    kind: "acp",
    title,
    state,
    elapsedMs: null,
    createdBy: "parent",
    activity,
  };
}

/** A child the parent created that finished with code 0. */
export function ended(id: string, generation = 1, title = id): Session {
  return rosterRow(
    id,
    { type: "ended", generation, code: 0, integrity: { kind: "complete" } },
    title,
  );
}

/** A child the parent created that finished with a failing exit code. */
export function failed(id: string, generation = 1, title = id): Session {
  return rosterRow(
    id,
    { type: "ended", generation, code: 1, integrity: { kind: "complete" } },
    title,
  );
}

/** A child the parent created whose turn is running. */
export function live(id: string, generation = 1, title = id): Session {
  return rosterRow(id, { type: "live", generation }, title, "working");
}

/** A child the parent created that is still up with its turn done. */
export function idle(id: string, generation = 1, title = id): Session {
  return rosterRow(id, { type: "live", generation }, title, "idle");
}

export function surface(
  roster: readonly Session[],
  refresh: () => Promise<void>,
  observedState?: SessionState,
  onOpenSubagent: (sessionId: string) => void = () => undefined,
) {
  return (
    <AgentChatSurface
      daemonState="connected"
      sessionId="parent"
      title="Parent"
      observedState={observedState ?? null}
      onOpenSubagent={onOpenSubagent}
      sessionRoster={roster}
      onRefreshSubagents={refresh}
    />
  );
}

/** The daemon's side of the roster: closing an id drops its row and pushes the
 * roster, and a test can push any roster of its own. */
export function closingDaemon(
  render: (node: ReactElement) => void,
  initial: readonly Session[],
  refresh: () => Promise<void>,
  observedState?: SessionState,
) {
  let roster = [...initial];
  const push = (next: readonly Session[]): void => {
    roster = [...next];
    render(surface(roster, refresh, observedState));
  };
  return {
    node: () => surface(roster, refresh, observedState),
    push,
    close: async (id: string): Promise<void> => push(roster.filter((row) => row.id !== id)),
  };
}

export async function addSubagent(id: string, title: string): Promise<void> {
  await act(async () => {
    channelHarness.active?.({ type: "agent_task_started", taskId: id, title, spawnDepth: 1 });
  });
}

export async function settleSubagent(
  id: string,
  status: "completed" | "failed" | "stopped",
): Promise<void> {
  await act(async () => {
    channelHarness.active?.({ type: "agent_task_notification", taskId: id, status });
  });
}

export async function openMenu(): Promise<void> {
  const pill = document.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
  if (pill === null) throw new Error("subagent pill did not render");
  await act(async () => {
    pill.click();
  });
}

export function archiveAction(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('[data-testid="subagent-archive"]');
}

export function rowTitles(): string[] {
  return [...document.querySelectorAll(".workspace-subagent-row")].map(
    (row) => row.textContent ?? "",
  );
}

export async function pressArchiveAction(): Promise<void> {
  const action = archiveAction();
  if (action === null) throw new Error("archive action did not render");
  action.focus({ preventScroll: true });
  await act(async () => {
    action.click();
  });
}

export function ask(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".confirm-dialog");
}

export async function answerAsk(label: "Archive" | "Cancel"): Promise<void> {
  const dialog = ask();
  if (dialog === null) throw new Error("the archive ask did not open");
  const button = [...dialog.querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === label,
  );
  if (button === undefined) throw new Error(`the ask has no ${label} button`);
  await act(async () => {
    button.click();
  });
}
