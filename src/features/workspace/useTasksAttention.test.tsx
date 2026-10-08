// @vitest-environment happy-dom
// The Tasks tab's dot: news is a task that finished or failed out of view, and
// the attach reply, a stop, or another session's list is never news.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { SessionTask } from "../../types/ipc";
import type { BackgroundTaskState } from "../../lib/backgroundTasks";
import { fakeTaskSource, type FakeTaskSource } from "./backgroundTaskSourceHarness";
import type { BackgroundTaskSource } from "./useBackgroundTaskState";
import { useTasksAttention } from "./useTasksAttention";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function Probe({ source, visible }: { source: BackgroundTaskSource | null; visible: boolean }) {
  return <span data-testid="unseen">{String(useTasksAttention(source, visible))}</span>;
}

async function show(source: FakeTaskSource | null, visible: boolean): Promise<void> {
  await act(async () => {
    root.render(<Probe source={source} visible={visible} />);
  });
}

/** Lands a list on the source, as the session's controller would. */
async function land(source: FakeTaskSource, list: BackgroundTaskState | null): Promise<void> {
  await act(async () => source.set(list));
}

/** What the hook returned on the last render. */
function unseen(): boolean {
  return container.querySelector('[data-testid="unseen"]')?.textContent === "true";
}

function task(id: string, overrides: Partial<SessionTask> = {}): SessionTask {
  return {
    id,
    kind: "agent",
    title: id,
    state: "running",
    sessionId: "parent-1",
    childSessionId: id,
    startedAtMs: 1_000,
    ...overrides,
  };
}

function list(revision: number, tasks: SessionTask[]): BackgroundTaskState {
  return { epoch: "e1", revision, tasks, omitted: 0 };
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

describe("the Tasks tab's attention dot", () => {
  it("is not lit by a list the source already held when the tab was watching", async () => {
    const source = fakeTaskSource(list(0, [task("a", { state: "finished", endedAtMs: 2_000 })]));
    await show(source, false);

    expect(unseen()).toBe(false);
  });

  it("is not lit by the attach reply that fills an empty list", async () => {
    const source = fakeTaskSource(null);
    await show(source, false);

    await land(source, list(0, [task("a", { state: "finished", endedAtMs: 2_000 })]));

    expect(unseen()).toBe(false);
  });

  it("lights up when a task finishes while the tab is out of view", async () => {
    const source = fakeTaskSource(list(1, [task("a")]));
    await show(source, false);

    await land(source, list(2, [task("a", { state: "finished", endedAtMs: 4_000 })]));

    expect(unseen()).toBe(true);
  });

  it("lights up when a task fails while the tab is out of view", async () => {
    const source = fakeTaskSource(list(1, [task("a")]));
    await show(source, false);

    await land(source, list(2, [task("a", { state: "failed", endedAtMs: 4_000 })]));

    expect(unseen()).toBe(true);
  });

  it("stays dark when a task is stopped, which the person asked for", async () => {
    const source = fakeTaskSource(list(1, [task("a")]));
    await show(source, false);

    await land(source, list(2, [task("a", { state: "cancelled", endedAtMs: 4_000 })]));

    expect(unseen()).toBe(false);
  });

  it("clears once the tab is in view", async () => {
    const source = fakeTaskSource(list(1, [task("a")]));
    await show(source, false);
    await land(source, list(2, [task("a", { state: "finished", endedAtMs: 4_000 })]));
    expect(unseen()).toBe(true);

    await show(source, true);

    expect(unseen()).toBe(false);
  });

  it("does not compare a source with another source's list", async () => {
    const first = fakeTaskSource(list(1, [task("a")]));
    await show(first, false);
    const second = fakeTaskSource(list(2, [task("a", { state: "finished", endedAtMs: 4_000 })]));

    await show(second, false);

    expect(unseen()).toBe(false);
  });
});
