// What the subagent archive's tests share: the surface they render, the
// roster rows they feed it, and the steps a person takes through the menu and
// its ask. Each test file keeps its own mount and its own `vi.mock`.
import { act } from "react";
import { channelHarness } from "./sessionChannelHarness";
import { AgentChatSurface } from "./AgentChatSurface";
import type { Session, SessionState } from "../../types/ipc";

function rosterRow(id: string, state: SessionState): Session {
  return { id, workspaceId: null, kind: "acp", title: id, state, elapsedMs: null };
}

export function ended(id: string, generation = 1): Session {
  return rosterRow(id, { type: "ended", generation, code: 0, integrity: { kind: "complete" } });
}

export function live(id: string, generation = 1): Session {
  return rosterRow(id, { type: "live", generation });
}

export function surface(
  roster: readonly Session[],
  refresh: () => Promise<void>,
  observedState?: SessionState,
) {
  return (
    <AgentChatSurface
      daemonState="connected"
      sessionId="parent"
      title="Parent"
      observedState={observedState ?? null}
      onOpenSubagent={() => undefined}
      sessionRoster={roster}
      subagentSessionIds={new Set(roster.map((row) => row.id))}
      onRefreshSubagents={refresh}
    />
  );
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
