// The goal row on the agent surface: pinned between the pane header and
// the transcript, seeded from the roster snapshot so a stopped session
// shows it, driven live by `goal_changed` afterwards, and gone with no goal.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionState } from "../../types/ipc";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const GOAL = "Move checkout to the provider registry without a slow first call";
const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "complete" },
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

async function renderSurface(props: {
  initialGoal?: string | null;
  observedState?: SessionState | null;
}): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        initialGoal={props.initialGoal}
        observedState={props.observedState ?? null}
      />,
    );
  });
  await act(async () => undefined);
}

async function rerenderSurface(props: {
  initialGoal?: string | null;
  observedState?: SessionState | null;
}): Promise<void> {
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        initialGoal={props.initialGoal}
        observedState={props.observedState ?? null}
      />,
    );
  });
  await act(async () => undefined);
}

function overflowGoalText(wide: boolean): void {
  const text = container.querySelector(".goal-line-text");
  if (text === null) throw new Error("goal text did not render");
  Object.defineProperty(text, "clientWidth", { value: 400, configurable: true });
  Object.defineProperty(text, "scrollWidth", {
    value: wide ? 800 : 400,
    configurable: true,
  });
}

function follows(earlier: Element | null, later: Element | null): boolean {
  if (earlier === null || later === null) throw new Error("surface row did not render");
  return (earlier.compareDocumentPosition(later) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
}

describe("the goal row on the agent surface", () => {
  it("sits between the pane header and the transcript, above the composer", async () => {
    await renderSurface({ initialGoal: GOAL });

    const header = container.querySelector(".workspace-agent-toolbar");
    const goal = container.querySelector('[data-testid="goal-line"]');
    const conversation = container.querySelector(".workspace-conversation");
    const composer = container.querySelector(".workspace-composer-track");
    expect(goal?.textContent).toContain(GOAL);
    expect(follows(header, goal)).toBe(true);
    expect(follows(goal, conversation)).toBe(true);
    expect(follows(conversation, composer)).toBe(true);
  });

  it("shows the roster seed on a stopped session", async () => {
    await renderSurface({ initialGoal: GOAL, observedState: ENDED });

    expect(container.querySelector('[data-testid="goal-line"]')?.textContent).toContain(GOAL);
  });

  it("shows a live goal_changed frame with no seed", async () => {
    await renderSurface({});
    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();

    await act(async () => {
      channelHarness.active?.({ type: "goal_changed", goal: GOAL });
    });

    expect(container.querySelector('[data-testid="goal-line"]')?.textContent).toContain(GOAL);
  });

  it("drops the row when the goal clears", async () => {
    await renderSurface({ initialGoal: GOAL });
    expect(container.querySelector('[data-testid="goal-line"]')).not.toBeNull();

    await act(async () => {
      channelHarness.active?.({ type: "goal_changed", goal: null });
    });

    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();
  });

  it("renders no row with no goal anywhere", async () => {
    await renderSurface({});

    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();
  });

  it("keeps a live goal across a resume with no new frame", async () => {
    await renderSurface({ observedState: { type: "live", generation: 1 } });
    await act(async () => {
      channelHarness.active?.({ type: "goal_changed", goal: GOAL });
    });
    expect(container.querySelector('[data-testid="goal-line"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="goal-line"]')?.textContent).toContain(GOAL);

    await rerenderSurface({ observedState: { type: "live", generation: 2 } });

    expect(container.querySelector('[data-testid="goal-line"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="goal-line"]')?.textContent).toContain(GOAL);
  });

  it("keeps a clear across a resume with a stale roster", async () => {
    await renderSurface({ initialGoal: "A", observedState: { type: "live", generation: 1 } });
    expect(container.querySelector('[data-testid="goal-line"]')?.textContent).toContain("A");

    await act(async () => {
      channelHarness.active?.({ type: "goal_changed", goal: "" });
    });
    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();

    await rerenderSurface({ initialGoal: "A", observedState: { type: "live", generation: 2 } });

    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();
  });

  it("adopts a roster goal that lands after mount on the next resume", async () => {
    await renderSurface({ observedState: { type: "live", generation: 1 } });
    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();

    await rerenderSurface({
      initialGoal: "ROSTER",
      observedState: { type: "live", generation: 1 },
    });
    // Live frames stay the source of truth while the controller lives.
    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();

    await rerenderSurface({
      initialGoal: "ROSTER",
      observedState: { type: "live", generation: 2 },
    });

    expect(container.querySelector('[data-testid="goal-line"]')?.textContent).toContain("ROSTER");
  });

  it("arrives collapsed when the goal is replaced", async () => {
    interface SeenObserver {
      callback: ResizeObserverCallback;
      target: Element | null;
    }
    const live: SeenObserver[] = [];
    vi.stubGlobal(
      "ResizeObserver",
      class implements SeenObserver {
        callback: ResizeObserverCallback;
        target: Element | null = null;
        constructor(callback: ResizeObserverCallback) {
          this.callback = callback;
          live.push(this);
        }
        observe(target: Element): void {
          this.target = target;
        }
        disconnect(): void {}
      },
    );
    const fireGoalObserver = (): void => {
      const seen = live.find(
        (observer) =>
          observer.target instanceof Element &&
          observer.target.classList.contains("goal-line-text"),
      );
      if (seen === undefined) throw new Error("goal observer did not install");
      act(() => seen.callback([], {} as ResizeObserver));
    };
    try {
      await renderSurface({ initialGoal: `First ${GOAL}` });
      overflowGoalText(true);
      fireGoalObserver();
      const toggle = container.querySelector<HTMLButtonElement>('[data-testid="goal-line-toggle"]');
      if (toggle === null) throw new Error("chevron did not render");
      await act(async () => toggle.click());
      expect(container.querySelector('[data-testid="goal-line"]')?.classList).toContain(
        "is-expanded",
      );

      await act(async () => {
        channelHarness.active?.({ type: "goal_changed", goal: `Second ${GOAL}` });
      });

      const row = container.querySelector('[data-testid="goal-line"]');
      expect(row?.textContent).toContain(`Second ${GOAL}`);
      expect(row?.classList).not.toContain("is-expanded");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
