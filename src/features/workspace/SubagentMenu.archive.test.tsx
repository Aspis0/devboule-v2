// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";
import {
  ended,
  live,
  surface,
  addSubagent,
  settleSubagent,
  openMenu,
  archiveAction,
  rowTitles,
  pressArchiveAction,
  ask,
  answerAsk,
} from "./subagentArchiveHarness";
import { sessionClose } from "../../lib/tauri";
import type { Session } from "../../types/ipc";

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

describe("the subagent menu's archive ask and its guard", () => {
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
    expect(sentence?.textContent).toBe("It changed since you asked, so it was left open.");
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
    expect(sentence?.textContent).toBe("It changed since you asked, so it was left open.");
  });

  it("says the child is running when it is running under the generation the ask took", async () => {
    let roster: Session[] = [ended("child-a")];
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface(roster, refresh)));
    await addSubagent("child-a", "First child");
    await settleSubagent("child-a", "completed");
    await openMenu();

    await pressArchiveAction();
    roster = [live("child-a")];
    await act(async () => root.render(surface(roster, refresh)));
    await answerAsk("Archive");

    expect(sessionClose).not.toHaveBeenCalled();
    const sentence = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(sentence?.textContent).toBe("It changed since you asked, so it was left open.");
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

  it("counts a target the roster no longer holds as closed, without a call or a sentence", async () => {
    let roster: Session[] = [ended("child-only")];
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface(roster, refresh)));
    await addSubagent("child-only", "Only child");
    await settleSubagent("child-only", "completed");
    await openMenu();

    await pressArchiveAction();
    roster = [];
    await act(async () => root.render(surface(roster, refresh)));
    await answerAsk("Archive");

    expect(sessionClose).not.toHaveBeenCalled();
    expect(container.querySelector('[data-testid="subagent-pill"]')).toBeNull();
    expect(document.querySelector(".workspace-subagent-row-failure")).toBeNull();
    expect(document.activeElement).not.toBe(document.body);
  });
});
