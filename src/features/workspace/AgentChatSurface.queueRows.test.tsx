// The queue rows wired at the surface: each action reaches the daemon as its
// own frame with the row's id, a send-now carries the subscription this view
// holds, and delete hands focus back to the composer. The keyboard and the
// permission rule are pinned in `AgentChatSurface.queue.test.tsx`.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { QueuedMessage, SessionEvent } from "../../types/ipc";

const harness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  nextSubscriptionId: 41,
  revision: 0,
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    harness.emit = onEvent;
    return {};
  }),
  sessionAttach: vi.fn(async () => harness.nextSubscriptionId++),
  sessionDetach: vi.fn(async () => undefined),
  sessionSend: vi.fn(async () => true),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionDeposit: vi.fn(async () => ({ sessionId: "agent-1", digest: "a", storedBytes: 1 })),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  sessionQueueAdd: vi.fn(async () => undefined),
  sessionQueueEdit: vi.fn(async () => undefined),
  sessionQueueRemove: vi.fn(async () => undefined),
  sessionQueueMove: vi.fn(async () => undefined),
  sessionQueueSendNow: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

import {
  sessionQueueEdit,
  sessionQueueMove,
  sessionQueueRemove,
  sessionQueueSendNow,
  sessionSend,
} from "../../lib/tauri";
import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const EPOCH = "0123456789abcdef0123456789abcdef";

let container: HTMLDivElement;
let root: Root;

function row(itemId: string, text: string, error?: string): QueuedMessage {
  return { itemId, text, ...(error === undefined ? {} : { error }) };
}

function pushSnapshot(items: readonly QueuedMessage[]): void {
  harness.revision += 1;
  act(() => {
    harness.emit?.({
      type: "queue_snapshot",
      epoch: EPOCH,
      revision: harness.revision,
      items: [...items],
    });
  });
}

async function renderSurface(items: readonly QueuedMessage[]) {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface daemonState="connected" sessionId="agent-1" title="Agent" queueSupported />,
    );
  });
  await act(async () => undefined);
  pushSnapshot(items);
  await act(async () => undefined);
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

async function flush(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  harness.emit = null;
  harness.nextSubscriptionId = 41;
  harness.revision = 0;
  localStorage.removeItem("devboule.sendBehavior");
  localStorage.removeItem("devboule.modelEffortPrefs");
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("AgentChatSurface queue rows", () => {
  it("edits the row in place through one frame", async () => {
    await renderSurface([row("queue-1", "first"), row("queue-2", "second")]);

    await act(async () => rowButton(rows()[1], "queue-edit").click());
    const input = container.querySelector<HTMLInputElement>('[data-testid="queue-edit-input"]');
    if (input === null) throw new Error("the row editor did not open");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    setValue.call(input, "second, edited");
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => rowButton(rows()[1], "queue-edit-save").click());
    await flush();

    expect(vi.mocked(sessionQueueEdit).mock.calls).toEqual([
      ["agent-1", expect.stringMatching(/^[0-9a-f-]{36}$/), "queue-2", "second, edited"],
    ]);
    // The row never left the queue, so the composer keeps what it had.
    expect(rows()).toHaveLength(2);
    expect(textarea().value).toBe("");
  });

  it("hands an edited row's text back when the daemon no longer has the row", async () => {
    await renderSurface([row("queue-1", "first")]);
    vi.mocked(sessionQueueEdit).mockRejectedValue({
      code: "invalid_request",
      message: "No queued message 'queue-1' is in this session's queue.",
    });

    await act(async () => rowButton(rows()[0], "queue-edit").click());
    const input = container.querySelector<HTMLInputElement>('[data-testid="queue-edit-input"]');
    if (input === null) throw new Error("the row editor did not open");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    setValue.call(input, "edited, and gone");
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => rowButton(rows()[0], "queue-edit-save").click());
    await flush();

    expect(textarea().value).toBe("edited, and gone");
  });

  it("renders the reason a refused send left on its row", async () => {
    await renderSurface([row("queue-1", "stuck", "The message was not sent.")]);
    const note = rows()[0].querySelector('[role="alert"]');
    expect(note?.textContent).toBe("The message was not sent.");
  });

  it("sends a row now with the subscription this view holds", async () => {
    await renderSurface([row("queue-1", "hold this")]);

    await act(async () => rowButton(rows()[0], "queue-steer").click());
    await flush();

    expect(vi.mocked(sessionQueueSendNow).mock.calls).toEqual([
      ["agent-1", expect.any(String), 41, "queue-1"],
    ]);
    // The interrupt is the daemon's now: this view sends nothing itself.
    expect(sessionSend).not.toHaveBeenCalled();
  });

  it("says a refused send-now once, beside the composer", async () => {
    await renderSurface([row("queue-1", "hold this")]);
    vi.mocked(sessionQueueSendNow).mockRejectedValue({
      code: "invalid_request",
      message: "A queued message is already being sent for this session.",
    });

    await act(async () => rowButton(rows()[0], "queue-steer").click());
    await flush();

    expect(sessionQueueSendNow).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[data-testid="queue-error"]')?.textContent).toBe(
      "The agent daemon refused that request as invalid.",
    );
    expect(rows()).toHaveLength(1);
  });

  it("when delete takes the last row, the composer takes the focus back", async () => {
    await renderSurface([row("queue-1", "only")]);

    rows()[0].focus();
    await act(async () => rowButton(rows()[0], "queue-delete").click());
    await act(async () => undefined);
    // The daemon removes the row: until its snapshot says so it is still there.
    expect(rows()).toHaveLength(1);
    expect(vi.mocked(sessionQueueRemove).mock.calls).toEqual([
      ["agent-1", expect.any(String), "queue-1"],
    ]);

    pushSnapshot([]);
    await act(async () => undefined);
    expect(container.querySelector('[data-testid="queue-track"]')).toBeNull();
    expect(document.activeElement).toBe(textarea());
  });

  it("reorder moves the row that can move, and asks for no place the queue has not got", async () => {
    await renderSurface([
      row("queue-1", "first"),
      row("queue-2", "middle"),
      row("queue-3", "last"),
    ]);

    // Top row up and bottom row down: both already stand where they are.
    await act(async () =>
      rows()[0].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true }),
      ),
    );
    await act(async () =>
      rows()[2].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", altKey: true, bubbles: true }),
      ),
    );
    await flush();
    expect(sessionQueueMove).not.toHaveBeenCalled();

    await act(async () =>
      rows()[2].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true }),
      ),
    );
    await flush();
    expect(vi.mocked(sessionQueueMove).mock.calls[0]).toEqual([
      "agent-1",
      expect.any(String),
      "queue-3",
      1,
    ]);
    expect(sessionSend).not.toHaveBeenCalled();
  });
});
