// The queue at the surface: the daemon's snapshot is what renders, Enter sends
// or queues by the setting, and nothing here ever sends a queued message.
// The rows' own actions are pinned in `AgentChatSurface.queueRows.test.tsx`,
// the hook's rules in `useMessageQueue.test.tsx`.
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
  sessionSend: vi.fn(async () => false),
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

import { sessionQueueAdd, sessionQueueSendNow, sessionSend } from "../../lib/tauri";
import { setSendBehavior } from "../../lib/sendBehavior";
import { AgentChatSurface, QUEUE_UNSUPPORTED } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SETTING_KEY = "devboule.sendBehavior";
const EPOCH = "0123456789abcdef0123456789abcdef";

let container: HTMLDivElement;
let root: Root;
let surfaceProps: Record<string, unknown> = {};

async function renderSurface(props: Record<string, unknown> = {}): Promise<void> {
  surfaceProps = { ...surfaceProps, ...props };
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        queueSupported
        {...surfaceProps}
      />,
    );
  });
  await act(async () => undefined);
}

function updateSurfaceProps(next: Record<string, unknown>): void {
  surfaceProps = { ...surfaceProps, ...next };
  act(() => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        queueSupported
        {...surfaceProps}
      />,
    );
  });
}

function row(itemId: string, text: string): QueuedMessage {
  return { itemId, text };
}

/** What the daemon publishes on the session's own channel. */
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

function textarea(): HTMLTextAreaElement {
  const element = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (element === null) throw new Error("composer textarea did not render");
  return element;
}

function type(text: string): void {
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");
  setValue.call(textarea(), text);
  textarea().dispatchEvent(new Event("input", { bubbles: true }));
}

function pressEnter(alternate = false): void {
  textarea().dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Enter",
      ctrlKey: alternate,
      metaKey: alternate,
      bubbles: true,
    }),
  );
}

async function clickSend(): Promise<void> {
  const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
  if (send === null) throw new Error("send button did not render");
  await act(async () => send.click());
}

function queueAction(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('[data-testid="composer-queue-action"]');
}

function rows(): NodeListOf<HTMLDivElement> {
  return container.querySelectorAll('[data-testid="queue-row"]');
}

function queueError(): string | null {
  return container.querySelector('[data-testid="queue-error"]')?.textContent ?? null;
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
  surfaceProps = {};
  localStorage.removeItem(SETTING_KEY);
  localStorage.removeItem("devboule.agentPrefs");
  // The store reads storage once at boot; tests drive it through its API.
  setSendBehavior("queue");
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("AgentChatSurface queue keys", () => {
  it("renders the daemon's rows and nothing before the first snapshot", async () => {
    await renderSurface();
    expect(container.querySelector('[data-testid="queue-track"]')).toBeNull();

    pushSnapshot([row("queue-1", "queued follow-up"), row("queue-2", "and another")]);
    expect(rows()).toHaveLength(2);
    expect(rows()[0].textContent).toContain("queued follow-up");
  });

  it("sends on Enter with no turn running, as today", async () => {
    await renderSurface();
    type("hello");
    await act(async () => pressEnter());
    expect(sessionSend).toHaveBeenCalledWith("agent-1", 41, "hello");
    expect(sessionQueueAdd).not.toHaveBeenCalled();
  });

  it("queues on Enter while the turn runs, and sends nothing at all", async () => {
    await renderSurface();
    type("first");
    await clickSend();
    expect(sessionSend).toHaveBeenCalledTimes(1);
    updateSurfaceProps({ activity: "working" });

    type("second");
    await act(async () => pressEnter());
    await flush();

    expect(vi.mocked(sessionQueueAdd).mock.calls).toEqual([
      ["agent-1", expect.stringMatching(/^[0-9a-f-]{36}$/), "second", [], []],
    ]);
    // The row is the daemon's to publish, and this surface sends no queued
    // text: that is the double-send gate.
    pushSnapshot([row("queue-1", "second")]);
    expect(rows()).toHaveLength(1);
    expect(rows()[0].textContent).toContain("second");
    expect(sessionSend).toHaveBeenCalledTimes(1);
    expect(textarea().value).toBe("");
  });

  it("steers on Ctrl+Enter as an ordinary send, outside the queue", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    updateSurfaceProps({ activity: "working" });

    type("jump in");
    await act(async () => pressEnter(true));
    await flush();

    // The daemon's send-now acts on a queued row; text that was never queued
    // steers the running turn directly.
    expect(vi.mocked(sessionSend).mock.calls[1]?.[2]).toBe("jump in");
    expect(sessionQueueAdd).not.toHaveBeenCalled();
    expect(sessionQueueSendNow).not.toHaveBeenCalled();
    expect(rows()).toHaveLength(0);
  });

  it("under the steer setting Enter steers and Ctrl+Enter queues", async () => {
    setSendBehavior("interrupt-and-send");
    await renderSurface();
    type("running now");
    await clickSend();
    updateSurfaceProps({ activity: "working" });

    type("from enter");
    await act(async () => pressEnter());
    await flush();
    expect(vi.mocked(sessionSend).mock.calls[1]?.[2]).toBe("from enter");
    expect(sessionQueueAdd).not.toHaveBeenCalled();

    type("queued instead");
    await act(async () => pressEnter(true));
    await flush();
    expect(vi.mocked(sessionQueueAdd).mock.calls[0]?.[2]).toBe("queued instead");
    expect(sessionSend).toHaveBeenCalledTimes(2);
  });

  it("under the steer setting, Ctrl+Enter with no running turn does nothing", async () => {
    setSendBehavior("interrupt-and-send");
    await renderSurface();
    type("hold on");
    await act(async () => pressEnter(true));
    await flush();
    expect(sessionSend).not.toHaveBeenCalled();
    expect(sessionQueueAdd).not.toHaveBeenCalled();
    expect(textarea().value).toBe("hold on");
  });

  it("with a permission card open, Enter steers and the button label says so", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    updateSurfaceProps({ activity: "working", hasPendingPermission: true });

    expect(queueAction()).toBeNull();

    type("behind a card");
    await act(async () => pressEnter());
    await flush();
    // Queueing here would strand the text behind a card nobody has answered.
    expect(vi.mocked(sessionSend).mock.calls[1]?.[2]).toBe("behind a card");
    expect(sessionQueueAdd).not.toHaveBeenCalled();
  });

  it("puts a refused plain send back into the composer", async () => {
    await renderSurface();
    vi.mocked(sessionSend).mockRejectedValueOnce(new Error("Session input is too large."));

    type("too big");
    await act(async () => pressEnter());
    await flush();

    // A refused send leaves the text in the composer: the user typed it, and
    // the refusal says why it did not go.
    expect(textarea().value).toBe("too big");
    expect(document.activeElement).toBe(textarea());
  });

  it("says a refused queueing beside the composer and hands the text back", async () => {
    await renderSurface();
    updateSurfaceProps({ activity: "working" });
    vi.mocked(sessionQueueAdd).mockRejectedValueOnce({
      code: "invalid_request",
      message: "A queued message needs text or an attachment.",
    });

    type("not accepted");
    await act(async () => pressEnter());
    await flush();

    expect(queueError()).toBe("The agent daemon refused that request as invalid.");
    expect(textarea().value).toBe("not accepted");
  });

  it("labels the composer action Queue message while the default queues", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    updateSurfaceProps({ activity: "working" });
    expect(queueAction()?.getAttribute("aria-label")).toBe("Queue message");
  });

  it("keeps the running-turn action button disabled while the composer is empty", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    updateSurfaceProps({ activity: "working" });
    expect(queueAction()?.disabled).toBe(true);
    type("   ");
    expect(queueAction()?.disabled).toBe(true);
    type("now it can queue");
    expect(queueAction()?.disabled).toBe(false);
  });

  it("on a daemon that does not own the queue: Enter sends, the action says why, no frame", async () => {
    await renderSurface({ queueSupported: false });
    updateSurfaceProps({ activity: "working" });
    type("a follow-up");
    await act(async () => pressEnter());
    await flush();

    // No queue frame can reach a daemon that cannot read one.
    expect(sessionQueueAdd).not.toHaveBeenCalled();
    expect(vi.mocked(sessionSend).mock.calls[0]?.[2]).toBe("a follow-up");
    expect(rows()).toHaveLength(0);

    // The action is still there, and it says the truth instead of lying.
    const action = queueAction();
    expect(action?.disabled).toBe(true);
    expect(action?.getAttribute("aria-label")).toBe(QUEUE_UNSUPPORTED);
  });
});
