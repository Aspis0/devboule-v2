// @vitest-environment happy-dom

// Children the parent created reach the pill from the roster alone: no task
// event fires in any test but the mixed list's, because none fires for them.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";
import {
  addSubagent,
  answerAsk,
  archiveAction,
  idle,
  live,
  openMenu,
  pressArchiveAction,
  rowTitles,
  settleSubagent,
  surface,
} from "./subagentArchiveHarness";
import { sessionClose } from "../../lib/tauri";

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

function pill(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
}

describe("a child the parent created, with no task event", () => {
  it("shows the child in the pill and lists it in the menu", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([idle("child-a", 1, "Created child")], refresh)));

    expect(pill()).not.toBeNull();
    await openMenu();
    expect(rowTitles()).toEqual(["Created child"]);
  });

  it("offers the archive for an idle child and closes it on confirm", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([idle("child-a", 1, "Created child")], refresh)));
    await openMenu();

    expect(archiveAction()?.textContent).toBe("Archive 1 finished subagent");
    await pressArchiveAction();
    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-a");
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it("counts a working child as working and does not offer the archive", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([live("child-a", 1, "Busy child")], refresh)));

    expect(pill()?.getAttribute("aria-label")).toBe("Subagents: 1 working");
    await openMenu();
    expect(archiveAction()).toBeNull();
  });

  it("does not list a session another agent created", async () => {
    const refresh = vi.fn(async () => undefined);
    const stranger = { ...idle("child-x", 1, "Not mine"), createdBy: "someone-else" };
    await act(async () => root.render(surface([stranger], refresh)));

    expect(pill()).toBeNull();
  });

  it("opens the child's session from its row", async () => {
    const refresh = vi.fn(async () => undefined);
    const opened = vi.fn();
    await act(async () =>
      root.render(surface([idle("child-a", 1, "Created child")], refresh, undefined, opened)),
    );
    await openMenu();

    const row = document.querySelector<HTMLButtonElement>(".workspace-subagent-row");
    expect(row?.disabled).toBe(false);
    await act(async () => {
      row?.click();
    });

    expect(opened).toHaveBeenCalledWith("child-a");
  });

  it("skips a child that went from idle to working while the ask was open", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () =>
      root.render(
        surface([idle("child-a", 1, "First child"), idle("child-b", 1, "Second child")], refresh),
      ),
    );
    await openMenu();

    await pressArchiveAction();
    await act(async () =>
      root.render(
        surface([live("child-a", 1, "First child"), idle("child-b", 1, "Second child")], refresh),
      ),
    );
    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-b");
    const sentence = document.querySelector<HTMLElement>(".workspace-subagent-row-failure");
    expect(sentence?.textContent).toBe("It changed since you asked, so it was left open.");
  });
});

describe("the list that mixes created children and provider tasks", () => {
  it("lists children first, then tasks, and archives only the child", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([idle("child-a", 1, "Created child")], refresh)));
    await addSubagent("task-1", "Provider task");
    await settleSubagent("task-1", "completed");
    await openMenu();

    expect(rowTitles()).toEqual(["Created child", "Provider taskUnavailable"]);
    expect(archiveAction()?.textContent).toBe("Archive 1 finished subagent");
    await pressArchiveAction();
    await answerAsk("Archive");

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-a");
  });

  it("never archives a finished task whose id equals a roster session's id", async () => {
    const refresh = vi.fn(async () => undefined);
    await act(async () => root.render(surface([live("same-id", 1, "Busy child")], refresh)));
    await addSubagent("same-id", "Provider task");
    await settleSubagent("same-id", "completed");
    await openMenu();

    expect(rowTitles()).toEqual(["Busy child", "Provider task"]);
    expect(archiveAction()).toBeNull();
  });
});
