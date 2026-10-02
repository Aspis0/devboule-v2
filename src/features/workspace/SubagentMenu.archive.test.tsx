// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";
import { AgentChatSurface } from "./AgentChatSurface";
import { sessionClose } from "../../lib/tauri";
import type { Session, SessionState } from "../../types/ipc";

vi.mock("../../lib/tauri", async () => ({
  ...(await import("./sessionChannelHarness")).tauriMock,
  sessionClose: vi.fn(async () => undefined),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  vi.mocked(sessionClose).mockReset();
  vi.mocked(sessionClose).mockImplementation(async () => undefined);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
});

function rosterRow(id: string, state: SessionState): Session {
  return { id, workspaceId: null, kind: "acp", title: id, state, elapsedMs: null };
}

function ended(id: string, generation = 1): Session {
  return rosterRow(id, { type: "ended", generation, code: 0, integrity: { kind: "complete" } });
}

function live(id: string, generation = 1): Session {
  return rosterRow(id, { type: "live", generation });
}

function surface(
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

async function addSubagent(id: string, title: string): Promise<void> {
  await act(async () => {
    channelHarness.active?.({ type: "agent_task_started", taskId: id, title, spawnDepth: 1 });
  });
}

async function settleSubagent(
  id: string,
  status: "completed" | "failed" | "stopped",
): Promise<void> {
  await act(async () => {
    channelHarness.active?.({ type: "agent_task_notification", taskId: id, status });
  });
}

async function openMenu(): Promise<void> {
  const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
  if (pill === null) throw new Error("subagent pill did not render");
  await act(async () => {
    pill.click();
  });
}

function archiveAction(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('[data-testid="subagent-archive"]');
}

function rowTitles(): string[] {
  return [...document.querySelectorAll(".workspace-subagent-row")].map(
    (row) => row.textContent ?? "",
  );
}

async function pressArchiveAction(): Promise<void> {
  const action = archiveAction();
  if (action === null) throw new Error("archive action did not render");
  action.focus({ preventScroll: true });
  await act(async () => {
    action.click();
  });
}

function ask(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".confirm-dialog");
}

async function answerAsk(label: "Archive" | "Cancel"): Promise<void> {
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

describe("the subagent menu's archive action", () => {
  it("stays hidden while no finished child can be archived", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([live("child-run")], refresh)));
    await addSubagent("child-run", "Working child");
    await openMenu();
    expect(archiveAction()).toBeNull();
  });

  it("stays hidden for a finished child the roster does not list", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([], refresh)));
    await addSubagent("child-gone", "Finished child");
    await settleSubagent("child-gone", "completed");
    await openMenu();
    expect(document.querySelector(".workspace-subagent-list")).not.toBeNull();
    expect(archiveAction()).toBeNull();
  });

  it("asks first, names the count and what the close does, and acts only on the answer", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () =>
      root.render(surface([ended("child-a"), ended("child-b"), live("child-run")], refresh)),
    );
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await addSubagent("child-b", "Second child");
    await settleSubagent("child-b", "completed");
    await addSubagent("child-run", "Working child");
    await openMenu();

    await pressArchiveAction();

    const dialog = ask();
    expect(dialog?.querySelector(".confirm-dialog-title")?.textContent).toBe(
      "Archive 2 finished subagents?",
    );
    expect(dialog?.querySelector(".confirm-dialog-body")?.textContent).toBe(
      "They leave this list and their tabs close. Their transcripts stay in History; their attached files are removed.",
    );
    // The safe answer holds the keyboard, and the act is the danger fill.
    expect(document.activeElement?.classList.contains("confirm-dialog-cancel")).toBe(true);
    expect(
      dialog
        ?.querySelector(".confirm-dialog-confirm")
        ?.classList.contains("confirm-dialog-confirm-danger"),
    ).toBe(true);
    expect(sessionClose).not.toHaveBeenCalled();

    await answerAsk("Cancel");
    expect(ask()).toBeNull();
    expect(sessionClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(archiveAction());

    await pressArchiveAction();
    await answerAsk("Archive");
    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-a");
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-b");
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalledWith("child-run");
  });

  it("names the count, sits first, and follows the roster the refresh publishes", async () => {
    let roster: Session[] = [ended("child-a"), ended("child-b"), live("child-run")];
    let rowsAtRefresh = -1;
    const refresh = vi.fn(async () => {
      rowsAtRefresh = document.querySelectorAll(".workspace-subagent-row").length;
      roster = roster.filter((row) => row.id !== "child-a" && row.id !== "child-b");
      root.render(surface(roster, refresh));
    });
    await act(async () => root.render(surface(roster, refresh)));
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await addSubagent("child-b", "Second child");
    await settleSubagent("child-b", "completed");
    await addSubagent("child-run", "Working child");
    await openMenu();

    const action = archiveAction();
    expect(action?.textContent).toBe("Archive 2 finished subagents");
    const buttons = document.querySelectorAll(".workspace-subagent-list button");
    expect(buttons[0]).toBe(action);

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-a");
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-b");
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalledWith("child-run");
    expect(refresh).toHaveBeenCalledTimes(1);
    // The roster read ran while every row was still up: nothing hides
    // before the operation settles.
    expect(rowsAtRefresh).toBe(3);
    expect(rowTitles()).toEqual(["Working child"]);
    expect(archiveAction()).toBeNull();
    expect(document.querySelector(".workspace-subagent-list")).not.toBeNull();
  });

  it("leaves a child alone that restarted while the ask was open", async () => {
    let roster: Session[] = [ended("child-a"), ended("child-b")];
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface(roster, refresh)));
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await addSubagent("child-b", "Second child");
    await settleSubagent("child-b", "completed");
    await openMenu();

    await pressArchiveAction();
    // A push lands while the ask is up: the same id is a running session
    // again, under a new generation.
    roster = [live("child-a", 2), ended("child-b")];
    await act(async () => root.render(surface(roster, refresh)));

    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-b");
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalledWith("child-a");
    // The restarted child keeps its row and says why it was left alone.
    expect(rowTitles()).toEqual(["First child"]);
    const sentence = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(sentence?.textContent).toBe("It restarted, so it was left open.");
  });

  it("leaves a child alone that restarted and finished again under a new generation", async () => {
    let roster: Session[] = [ended("child-a"), ended("child-b")];
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface(roster, refresh)));
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await addSubagent("child-b", "Second child");
    await settleSubagent("child-b", "completed");
    await openMenu();

    await pressArchiveAction();
    // A restart and a fresh finish both land while the ask is up: same
    // finished state, a different generation of the same id.
    roster = [ended("child-a", 2), ended("child-b")];
    await act(async () => root.render(surface(roster, refresh)));

    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-b");
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalledWith("child-a");
    expect(rowTitles()).toEqual(["First child"]);
    const sentence = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(sentence?.textContent).toBe("It restarted, so it was left open.");
  });

  it("closes each child once when the confirm button is activated twice in one tick", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([ended("child-a"), ended("child-b")], refresh)));
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await addSubagent("child-b", "Second child");
    await settleSubagent("child-b", "completed");
    await openMenu();

    await pressArchiveAction();
    const dialog = ask();
    const confirm = [...(dialog?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
      (candidate) => candidate.textContent === "Archive",
    );
    if (confirm === undefined) throw new Error("the ask has no Archive button");
    await act(async () => {
      confirm.click();
      confirm.click();
    });

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-a");
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-b");
  });

  it("keeps a child whose close failed, with a mapped sentence and no raw daemon text", async () => {
    const refresh = vi.fn(async () => undefined);
    vi.mocked(sessionClose).mockImplementation(async (id: string) => {
      if (id === "child-bad") {
        throw { code: "journal", message: "raw journal words" };
      }
    });
    await act(async () => root.render(surface([ended("child-ok"), ended("child-bad")], refresh)));
    await addSubagent("child-ok", "Good child");
    await settleSubagent("child-ok", "completed");
    await addSubagent("child-bad", "Bad child");
    await settleSubagent("child-bad", "completed");
    await openMenu();

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(rowTitles()).toEqual(["Bad child"]);
    const failure = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(failure?.textContent).toBe("Saved history could not be read or written.");
    // The daemon's own words never reach the DOM, in any form.
    expect(document.body.textContent).not.toContain("raw journal words");
    expect(document.querySelector('[title*="raw journal"]')).toBeNull();
    expect(document.querySelector(".error-detail-sr-only")).toBeNull();
    // The child stayed in the roster, so its row still offers the action.
    expect(archiveAction()?.textContent).toBe("Archive 1 finished subagent");
  });

  it("keeps the act's answers when the roster read fails", async () => {
    const refresh = vi.fn(async () => Promise.reject(new Error("roster read failed")));
    vi.mocked(sessionClose).mockImplementation(async (id: string) => {
      if (id === "child-bad") {
        throw { code: "journal", message: "raw journal words" };
      }
    });
    await act(async () => root.render(surface([ended("child-ok"), ended("child-bad")], refresh)));
    await addSubagent("child-ok", "Good child");
    await settleSubagent("child-ok", "completed");
    await addSubagent("child-bad", "Bad child");
    await settleSubagent("child-bad", "completed");
    await openMenu();

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(refresh).toHaveBeenCalledTimes(1);
    expect(rowTitles()).toEqual(["Bad child"]);
    const failure = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(failure?.textContent).toBe("Saved history could not be read or written.");
  });

  it("leaves focus on the first remaining row", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () =>
      root.render(surface([ended("child-a"), ended("child-b"), live("child-run")], refresh)),
    );
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await addSubagent("child-b", "Second child");
    await settleSubagent("child-b", "completed");
    await addSubagent("child-run", "Working child");
    await openMenu();

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(document.querySelector(".workspace-subagent-list")).not.toBeNull();
    expect(document.activeElement?.classList.contains("workspace-subagent-row")).toBe(true);
    expect(document.activeElement?.textContent).toBe("Working child");
  });

  it("sends focus to the composer when the last child was archived", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([ended("child-only")], refresh)));
    await act(async () => {
      channelHarness.active?.({ type: "agent_finished", stopReason: "end_turn" });
    });
    await addSubagent("child-only", "Only child");
    await settleSubagent("child-only", "completed");
    await openMenu();
    const composer = document.querySelector<HTMLTextAreaElement>(".workspace-composer textarea");
    if (composer === null) throw new Error("composer did not render");

    await pressArchiveAction();
    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-only");
    expect(document.querySelector(".workspace-subagent-list")).toBeNull();
    expect(container.querySelector('[data-testid="subagent-pill"]')).toBeNull();
    expect(document.activeElement).toBe(composer);
    expect(document.activeElement).not.toBe(document.body);
  });

  it("takes the archived child out of the pill's counts", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([ended("child-fail"), live("child-run")], refresh)));
    await addSubagent("child-fail", "Failing child");
    await settleSubagent("child-fail", "failed");
    await addSubagent("child-run", "Working child");
    await openMenu();

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(pill?.getAttribute("aria-label")).toBe("Subagents: 1 failed, 1 working");

    await pressArchiveAction();
    await answerAsk("Archive");

    const after = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(after?.getAttribute("aria-label")).toBe("Subagents: 1 working");
    expect(rowTitles()).toEqual(["Working child"]);
  });

  it("keeps focus off the body when the composer cannot take it", async () => {
    const refresh = vi.fn(async () => undefined);
    const observedState: SessionState = {
      type: "ended",
      generation: 1,
      code: 0,
      integrity: { kind: "complete" },
    };
    await act(async () => root.render(surface([ended("child-only")], refresh, observedState)));
    await addSubagent("child-only", "Only child");
    await settleSubagent("child-only", "completed");
    await openMenu();
    const composer = document.querySelector<HTMLTextAreaElement>(".workspace-composer textarea");
    if (composer === null) throw new Error("composer did not render");
    expect(composer.disabled).toBe(true);

    await pressArchiveAction();
    await answerAsk("Archive");

    const transcript = document.querySelector<HTMLElement>(".workspace-conversation");
    expect(document.activeElement).toBe(transcript);
    expect(document.activeElement).not.toBe(document.body);
  });
});
