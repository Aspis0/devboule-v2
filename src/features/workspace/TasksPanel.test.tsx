// @vitest-environment happy-dom
// The Tasks tab's rows: their order and words, what each row offers, the stop
// that asks first, and the one clock that runs only while something runs.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionTask } from "../../types/ipc";
import type { AgentTasksContext } from "./sidePanelRegistry";

const confirm = vi.hoisted(() => ({ answer: true }));
const clock = vi.hoisted(() => ({ reads: 0 }));

// Counts the duration reads a render makes; the formatter itself is unchanged.
vi.mock("../../lib/backgroundTaskText", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/backgroundTaskText")>();
  return {
    ...actual,
    formatTaskDuration: (milliseconds: number) => {
      clock.reads += 1;
      return actual.formatTaskDuration(milliseconds);
    },
  };
});
const askConfirm = vi.hoisted(() => vi.fn(async () => confirm.answer));

vi.mock("../../components/ConfirmHost", () => ({
  useConfirmAsk: () => askConfirm,
}));

import { fakeTaskSource } from "./backgroundTaskSourceHarness";
import { TasksPanel } from "./TasksPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function task(overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id: "child-1",
    kind: "agent",
    title: "Explore auth",
    state: "running",
    sessionId: "parent-1",
    childSessionId: "child-1",
    startedAtMs: 1_000,
    ...overrides,
  };
}

function context(
  tasks: SessionTask[],
  omitted = 0,
  onStopAgent: AgentTasksContext["onStopAgent"] = async () => null,
  onOpenAgent: AgentTasksContext["onOpenAgent"] = () => null,
): AgentTasksContext {
  return {
    sessionId: "parent-1",
    source: fakeTaskSource({ epoch: "e1", revision: 1, tasks, omitted }),
    onOpenAgent,
    onStopAgent,
  };
}

async function render(tasks: AgentTasksContext | null): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<TasksPanel tasks={tasks} />);
  });
}

/** The rows' titles, in the order the panel draws them. */
function titles(): string[] {
  return Array.from(container.querySelectorAll(".tasks-panel-title")).map(
    (node) => node.textContent ?? "",
  );
}

function rowFor(title: string): HTMLLIElement {
  const row = Array.from(container.querySelectorAll<HTMLLIElement>(".tasks-panel-row")).find(
    (candidate) => candidate.textContent?.includes(title),
  );
  if (row === undefined) throw new Error(`no row for ${title}`);
  return row;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  confirm.answer = true;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("the Tasks tab's rows", () => {
  it("says so when no agent session is in the front pane", async () => {
    await render(null);

    expect(container.textContent).toContain("No background tasks");
  });

  it("says so when the session has no task", async () => {
    await render(context([]));

    expect(container.textContent).toContain("No background tasks");
  });

  it("lists the running rows first, then the settled ones, each newest first", async () => {
    await render(
      context([
        task({ id: "a", title: "Older run", startedAtMs: 1_000 }),
        task({ id: "b", title: "Newer run", startedAtMs: 9_000 }),
        task({
          id: "c",
          title: "Finished early",
          state: "finished",
          startedAtMs: 1_000,
          endedAtMs: 5_000,
        }),
        task({
          id: "d",
          title: "Failed late",
          state: "failed",
          startedAtMs: 1_000,
          endedAtMs: 20_000,
        }),
      ]),
    );

    expect(titles()).toEqual(["Newer run", "Older run", "Failed late", "Finished early"]);
    expect(container.textContent).toContain("Running");
    expect(container.textContent).toContain("Finished");
  });

  it("gives a failed row and a stopped row their state word under Finished", async () => {
    await render(
      context([
        task({ id: "f", title: "Broke", state: "failed", endedAtMs: 2_000 }),
        task({ id: "s", title: "Halted", state: "cancelled", endedAtMs: 2_000 }),
      ]),
    );

    expect(rowFor("Broke").textContent).toContain("Failed");
    expect(rowFor("Halted").textContent).toContain("Stopped");
  });

  it("shows the model and the tool count of a running agent when the daemon gave them", async () => {
    await render(context([task({ model: "test-model", toolCallCount: 1 })]));

    expect(rowFor("Explore auth").textContent).toContain("test-model");
    expect(rowFor("Explore auth").textContent).toContain("1 tool");
  });

  it("says how many rows the publish cap left out", async () => {
    await render(context([task()], 3));

    expect(container.textContent).toContain("+3 more");
  });

  it("says the cap left rows out even when none is shown", async () => {
    await render(context([], 2));

    expect(container.textContent).toContain("+2 more");
    expect(container.textContent).not.toContain("No background tasks");
  });
});

describe("what a row offers", () => {
  it("opens an agent's transcript from its title", async () => {
    const onOpenAgent = vi.fn();
    await render(context([task()], 0, undefined, onOpenAgent));

    rowFor("Explore auth").querySelector<HTMLButtonElement>("button.tasks-panel-title")?.click();

    expect(onOpenAgent).toHaveBeenCalledWith("child-1");
  });

  it("says why a child cannot be opened, in the alert line", async () => {
    await render(context([task()], 0, undefined, () => "This agent was archived."));

    rowFor("Explore auth").querySelector<HTMLButtonElement>("button.tasks-panel-title")?.click();
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe("This agent was archived.");
  });

  it("gives a command no link", async () => {
    await render(context([task({ id: "cmd-1", kind: "command", title: "npm test" })]));

    expect(rowFor("npm test").querySelector("button")).toBeNull();
  });

  it("offers Stop only on a running agent", async () => {
    await render(
      context([
        task({ id: "run", title: "Running agent" }),
        task({ id: "cmd", kind: "command", title: "Running command" }),
        task({ id: "done", title: "Done agent", state: "finished", endedAtMs: 4_000 }),
      ]),
    );

    expect(rowFor("Running agent").querySelector(".tasks-panel-stop")).not.toBeNull();
    expect(rowFor("Running command").querySelector(".tasks-panel-stop")).toBeNull();
    expect(rowFor("Done agent").querySelector(".tasks-panel-stop")).toBeNull();
  });

  it("asks before it stops, and stops nothing when the person keeps it running", async () => {
    confirm.answer = false;
    const onStopAgent = vi.fn(async () => null);
    await render(context([task()], 0, onStopAgent));

    rowFor("Explore auth").querySelector<HTMLButtonElement>(".tasks-panel-stop")?.click();
    await act(async () => undefined);

    expect(askConfirm).toHaveBeenCalledWith(expect.objectContaining({ confirmLabel: "Stop" }));
    expect(onStopAgent).not.toHaveBeenCalled();
  });

  it("stops the child once the person confirms", async () => {
    const onStopAgent = vi.fn(async () => null);
    await render(context([task()], 0, onStopAgent));

    rowFor("Explore auth").querySelector<HTMLButtonElement>(".tasks-panel-stop")?.click();
    await act(async () => undefined);

    expect(onStopAgent).toHaveBeenCalledWith("child-1");
  });

  it("does not stop a child twice while the first stop is still pending", async () => {
    let finish: (sentence: string | null) => void = () => undefined;
    const onStopAgent = vi.fn(() => new Promise<string | null>((resolve) => (finish = resolve)));
    await render(context([task()], 0, onStopAgent));
    const stop = () => rowFor("Explore auth").querySelector<HTMLButtonElement>(".tasks-panel-stop");

    // Both presses land before React draws the disabled button.
    await act(async () => {
      stop()?.click();
      stop()?.click();
    });
    await act(async () => undefined);

    expect(onStopAgent).toHaveBeenCalledTimes(1);
    expect(stop()?.disabled).toBe(true);

    await act(async () => finish(null));
    expect(stop()?.disabled).toBe(false);
  });

  it("shows the sentence the stop came back with, when it did not stop", async () => {
    const onStopAgent = vi.fn(async () => "It is no longer running, so nothing was stopped.");
    await render(context([task()], 0, onStopAgent));

    rowFor("Explore auth").querySelector<HTMLButtonElement>(".tasks-panel-stop")?.click();
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "It is no longer running, so nothing was stopped.",
    );
  });
});

describe("the duration clock", () => {
  it("reads the clock for the running row alone on each tick", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(66_000));
    await render(
      context([
        task({ startedAtMs: 1_000 }),
        task({
          id: "done",
          title: "Done",
          state: "finished",
          startedAtMs: 1_000,
          endedAtMs: 5_000,
        }),
      ]),
    );
    clock.reads = 0;

    await act(async () => {
      vi.advanceTimersByTime(1_000);
    });

    expect(clock.reads).toBe(1);
  });

  it("counts a running row up once a second", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(66_000));
    await render(context([task({ startedAtMs: 1_000 })]));
    expect(rowFor("Explore auth").textContent).toContain("1m 5s");

    await act(async () => {
      vi.advanceTimersByTime(1_000);
    });

    expect(rowFor("Explore auth").textContent).toContain("1m 6s");
  });

  it("shows no duration for a settled row that ended before it started", async () => {
    await render(context([task({ state: "finished", startedAtMs: 9_000, endedAtMs: 1_000 })]));

    expect(rowFor("Explore auth").querySelector(".tasks-panel-meta")).toBeNull();
  });

  it("shows a settled row's duration once, from its own start and end", async () => {
    await render(context([task({ state: "finished", startedAtMs: 1_000, endedAtMs: 89_000 })]));

    expect(rowFor("Explore auth").textContent).toContain("1m 28s");
  });
});
