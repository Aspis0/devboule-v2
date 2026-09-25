// The queue rows wired at the surface: edit hands the row's text back once
// the queue has taken the row, a refused send leaves its reason on the row,
// a row steer interrupts the running turn, delete hands focus back to the
// composer, and reorder moves what it should. The keyboard and the permission
// rule are pinned in `AgentChatSurface.queue.test.tsx`.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";
import { idleSender } from "./queueTestKit";
import { SEND_FAILED } from "./inMemoryMessageQueue";
import type { MessageQueue } from "./messageQueue";

const harness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  nextSubscriptionId: 41,
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    harness.emit = onEvent;
    return {};
  }),
  sessionAttach: vi.fn(async () => harness.nextSubscriptionId++),
  sessionDetach: vi.fn(async () => undefined),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

import { sessionInterrupt, sessionSend } from "../../lib/tauri";
import { AgentChatSurface } from "./AgentChatSurface";
import { createInMemoryMessageQueue } from "./inMemoryMessageQueue";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

async function renderSurface(queue: MessageQueue) {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface daemonState="connected" sessionId="agent-1" title="Agent" queue={queue} />,
    );
  });
  await act(async () => undefined);
  return queue;
}

function textarea(): HTMLTextAreaElement {
  const element = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (element === null) throw new Error("composer textarea did not render");
  return element;
}

function rows(): NodeListOf<HTMLDivElement> {
  return container.querySelectorAll('[data-testid="queue-row"]');
}

function rowButton(row: HTMLDivElement, testId: string): HTMLButtonElement {
  const button = row.querySelector<HTMLButtonElement>(`[data-testid="${testId}"]`);
  if (button === null) throw new Error(`${testId} button did not render`);
  return button;
}

/** Send from the composer, so the surface reports a running turn. */
async function startTurn(): Promise<void> {
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");
  setValue.call(textarea(), "running now");
  textarea().dispatchEvent(new Event("input", { bubbles: true }));
  const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
  if (send === null) throw new Error("send button did not render");
  await act(async () => send.click());
}

async function flush(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  harness.emit = null;
  harness.nextSubscriptionId = 41;
  localStorage.removeItem("devboule.sendBehavior");
  localStorage.removeItem("devboule.modelEffortPrefs");
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("AgentChatSurface queue rows", () => {
  it("edit takes the row out and puts its text back into the composer", async () => {
    const queue = await renderSurface(createInMemoryMessageQueue("agent-1", idleSender()));
    act(() => {
      queue.add("first", []);
      queue.add("second", []);
    });

    await act(async () => rowButton(rows()[1], "queue-edit").click());
    await flush();
    expect(textarea().value).toBe("second");
    expect(rows()).toHaveLength(1);
    expect(rows()[0].textContent).toContain("first");
  });

  it("renders the reason a refused send left on its row", async () => {
    const queue = await renderSurface(createInMemoryMessageQueue("agent-1", idleSender()));
    act(() => {
      queue.add("stuck", []);
      queue.setTurnStatus("idle");
    });
    vi.mocked(sessionSend).mockRejectedValueOnce(new Error("the session refused the send"));

    await act(async () => rowButton(rows()[0], "queue-steer").click());
    await flush();

    expect(rows()).toHaveLength(1);
    const note = rows()[0].querySelector('[role="alert"]');
    expect(note?.textContent).toBe(SEND_FAILED);
  });

  it("sends a row's own steer after the turn it interrupted is over", async () => {
    const queue = await renderSurface(createInMemoryMessageQueue("agent-1", idleSender()));
    act(() => {
      queue.add("hold this", []);
    });
    await startTurn();
    // The roster says a turn owns the session — the owner's one input to whether
    // a press interrupts first (fix-4: no second path guesses from frames).
    queue.setTurnStatus("working");

    await act(async () => rowButton(rows()[0], "queue-steer").click());
    await flush();
    expect(sessionInterrupt).toHaveBeenCalledTimes(1);
    expect(sessionInterrupt).toHaveBeenCalledWith("agent-1", 41);
    // The row waits: the cancel is only dispatched, and a send that followed it
    // straight away could reach a provider that has not stopped (review F6).
    expect(sessionSend).toHaveBeenCalledTimes(1);
    expect(rows()).toHaveLength(1);

    // The turn it interrupted closes and the row reads "idle": the predicate
    // falls, and that is what starts the drain.
    queue.setTurnStatus("idle");
    queue.notifyIdle();
    await flush();
    expect(sessionSend).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionSend).mock.calls[1]?.[2]).toBe("hold this");
    expect(vi.mocked(sessionSend).mock.calls[1]?.[6]).toMatch(/^agent-1\.queued-1\.[0-9a-f-]{36}$/);
    expect(rows()).toHaveLength(0);
  });

  it("when delete takes the last row, the composer takes the focus back", async () => {
    const queue = await renderSurface(createInMemoryMessageQueue("agent-1", idleSender()));
    act(() => {
      queue.add("only", []);
    });

    rows()[0].focus();
    await act(async () => rowButton(rows()[0], "queue-delete").click());
    await flush();
    expect(container.querySelector('[data-testid="queue-track"]')).toBeNull();
    expect(document.activeElement).toBe(textarea());
  });

  it("reorder moves the row that can move, and clamps the ones at the ends", async () => {
    const queue = await renderSurface(createInMemoryMessageQueue("agent-1", idleSender()));
    act(() => {
      queue.add("first", []);
      queue.add("middle", []);
      queue.add("last", []);
    });

    // Top row up and bottom row down: both clamp to no movement.
    await act(async () =>
      rows()[0].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true }),
      ),
    );
    await flush();
    await act(async () =>
      rows()[2].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", altKey: true, bubbles: true }),
      ),
    );
    await flush();
    expect(rows()).toHaveLength(3);
    expect(rows()[0].textContent).toContain("first");
    expect(rows()[2].textContent).toContain("last");

    // The bottom row moves up: the keyboard reaches the queue.
    await act(async () =>
      rows()[2].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true }),
      ),
    );
    await flush();
    expect(rows()[1].textContent).toContain("last");
    expect(rows()[2].textContent).toContain("middle");
    expect(sessionSend).not.toHaveBeenCalled();
  });
});
