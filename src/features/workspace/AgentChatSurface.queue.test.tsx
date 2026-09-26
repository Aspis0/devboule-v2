// The queue at the surface: Enter/Ctrl+Enter under both settings, the
// permission rule, the session bound to the queue it drains into, and a
// refused steer handing the text back. The rows' own actions are pinned in
// `AgentChatSurface.queueRows.test.tsx`; the queue's own rules in
// `inMemoryMessageQueue.*.test.ts`.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";
import { idleSender, sessionOf } from "./queueTestKit";
import { SEND_FAILED } from "./inMemoryMessageQueue";
import type { MessageQueue, QueuedMessage } from "./messageQueue";
import { createSessionQueueOwner, type SessionQueueOwner } from "./sessionQueueOwner";

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
import { setSendBehavior } from "../../lib/sendBehavior";
import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SETTING_KEY = "devboule.sendBehavior";

let container: HTMLDivElement;
let root: Root;
let queueOwner: SessionQueueOwner | null = null;
let surfaceQueue: MessageQueue | null = null;
let surfaceProps: Record<string, unknown> = {};

/** The queue as it stands, read through a throwaway subscription. */
function queuedTexts(queue: MessageQueue): string[] {
  let items: readonly QueuedMessage[] = [];
  const stop = queue.subscribe((next) => {
    items = next;
  });
  stop();
  return items.map((item) => item.text);
}

async function renderSurface(): Promise<MessageQueue> {
  queueOwner = createSessionQueueOwner({ newSender: () => idleSender() });
  const queue = queueOwner.queueFor("agent-1");
  surfaceQueue = queue;
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface daemonState="connected" sessionId="agent-1" title="Agent" queue={queue} />,
    );
  });
  await act(async () => undefined);
  return queue;
}

function pushActivity(activity: "working" | "blocked" | "idle" | "unknown"): void {
  act(() => queueOwner?.onRosterPush([sessionOf("agent-1", { activity })]));
  if (activity === "idle") {
    act(() => harness.emit?.({ type: "agent_finished", stopReason: "end_turn" }));
  }
}

function updateSurfaceProps(next: Record<string, unknown>): void {
  surfaceProps = { ...surfaceProps, ...next };
  act(() => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        queue={surfaceQueue}
        {...surfaceProps}
      />,
    );
  });
}

/** A surface with no queue handed over. The workspace always hands one —
 * this pins the fallback, not the shipped shape. */
async function renderSurfaceWithoutQueue(props: Record<string, unknown> = {}): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        queue={null}
        {...props}
      />,
    );
  });
  await act(async () => undefined);
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

function queueAction(): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>(
    '[data-testid="composer-queue-action"]',
  );
  if (button === null) throw new Error("composer queue action did not render");
  return button;
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
  queueOwner = null;
  surfaceQueue = null;
  surfaceProps = {};
  localStorage.removeItem(SETTING_KEY);
  localStorage.removeItem("devboule.modelEffortPrefs");
  // The store reads storage once at boot; tests drive it through its API.
  setSendBehavior("queue");
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("AgentChatSurface queue keys", () => {
  it("renders no queue track while the queue is empty", async () => {
    await renderSurface();
    expect(container.querySelector('[data-testid="queue-track"]')).toBeNull();
  });

  it("renders no rows and no queue action when no queue was handed over", async () => {
    await renderSurfaceWithoutQueue();
    type("running now");
    await clickSend();
    expect(container.querySelector('[data-testid="queue-track"]')).toBeNull();
    expect(container.querySelector('[data-testid="composer-queue-action"]')).toBeNull();
  });

  it("sends on Enter with no turn running, as today", async () => {
    const queue = await renderSurface();
    type("hello");
    await act(async () => pressEnter());
    expect(sessionSend).toHaveBeenCalledWith("agent-1", 41, "hello");
    expect(queuedTexts(queue)).toEqual([]);
  });

  it("uses roster activity when the surface has not rendered a streaming turn", async () => {
    const queue = await renderSurface();
    pushActivity("working");
    type("follow-up");

    expect(queueAction().textContent).toBe("Queue message");
    await act(async () => pressEnter());
    expect(queuedTexts(queue)).toEqual(["follow-up"]);
    expect(sessionSend).not.toHaveBeenCalled();
  });

  it("queues on Enter while the turn runs — the default Enter action", async () => {
    const queue = await renderSurface();
    type("first");
    await clickSend();
    expect(sessionSend).toHaveBeenCalledTimes(1);
    pushActivity("working");

    type("second");
    await act(async () => pressEnter());
    await flush();
    expect(queuedTexts(queue)).toEqual(["second"]);
    expect(sessionSend).toHaveBeenCalledTimes(1);
    expect(rows()).toHaveLength(1);
    expect(rows()[0].textContent).toContain("second");
    expect(textarea().value).toBe("");
  });

  it("uses an in-flight submission as active while the roster still says idle", async () => {
    const queue = await renderSurface();
    pushActivity("idle");
    let refuseSend!: (cause: Error) => void;
    vi.mocked(sessionSend).mockImplementationOnce(
      () => new Promise<boolean>((_resolve, reject) => (refuseSend = reject)),
    );
    type("first");
    await clickSend();

    type("second");
    await act(async () => pressEnter());

    expect(queuedTexts(queue)).toEqual(["second"]);
    expect(sessionSend).toHaveBeenCalledTimes(1);

    // Refused: no turn opens and no roster edge comes. The settle is the edge,
    // so "second" goes rather than sitting on an idle row (review fix-7
    // finding 1a, finding 11).
    await act(async () => refuseSend(new Error("refused")));
    await flush();
    expect(sessionSend).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionSend).mock.calls[1]?.[2]).toBe("second");
    expect(queuedTexts(queue)).toEqual([]);
  });

  it("shares one in-flight counter between two surfaces on the same session", async () => {
    const queue = await renderSurface();
    pushActivity("idle");
    const other = document.createElement("div");
    document.body.appendChild(other);
    const otherRoot = createRoot(other);
    await act(async () => {
      otherRoot.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="agent-1"
          title="Agent"
          queue={queue}
        />,
      );
    });
    const otherQueueAction = () => other.querySelector('[data-testid="composer-queue-action"]');
    expect(otherQueueAction()).toBeNull();

    let acceptSend!: (turnStarted: boolean) => void;
    vi.mocked(sessionSend).mockImplementationOnce(
      () => new Promise<boolean>((resolve) => (acceptSend = resolve)),
    );
    type("first");
    await clickSend();
    // The other pane sent nothing, and still sees this pane's send in flight
    // (review fix-7 finding 4).
    expect(otherQueueAction()?.textContent).toBe("Queue message");

    await act(async () => acceptSend(true));
    await flush();
    expect(otherQueueAction()?.textContent).toBe("Queue message");
    await act(async () => harness.emit?.({ type: "agent_finished", stopReason: "end_turn" }));
    await flush();
    expect(otherQueueAction()).toBeNull();
    await act(async () => otherRoot.unmount());
    other.remove();
  });

  it("steers on Ctrl+Enter: the interrupt goes now, the text after the turn is over", async () => {
    const queue = await renderSurface();
    type("running now");
    await clickSend();
    // The daemon's row for a session that has just taken a prompt says "working",
    // and the owner writes that reading onto the queue: it is the only thing a
    // steer defers to now (fix-4's single rule).
    pushActivity("working");

    type("jump in");
    await act(async () => pressEnter(true));
    await flush();
    expect(sessionInterrupt).toHaveBeenCalledTimes(1);
    expect(sessionInterrupt).toHaveBeenCalledWith("agent-1", 41);
    // The steer is ordered: the cancel goes out and the text waits at the front
    // of the queue, because the interrupt RPC answers when the cancel is
    // dispatched, not when the provider stopped (review F6).
    expect(sessionSend).toHaveBeenCalledTimes(1);
    expect(queuedTexts(queue)).toEqual(["jump in"]);
    expect(rows()).toHaveLength(1);

    // The turn that was interrupted closes and the row reads "idle": the
    // predicate falls, and that is what starts the drain.
    pushActivity("idle");
    await flush();
    expect(sessionSend).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionSend).mock.calls[1]?.[2]).toBe("jump in");
    // The retry identity rides the wire: this session, this queue item, and
    // this item's own nonce — the part that keeps a later queue for the same
    // session out of the first one's receipt (review fix-1 P2-3).
    expect(vi.mocked(sessionSend).mock.calls[1]?.[6]).toMatch(/^agent-1\.queued-1\.[0-9a-f-]{36}$/);
    expect(queuedTexts(queue)).toEqual([]);
    expect(textarea().value).toBe("");
  });

  it("leaves a steer the session refused on its row, and says it once", async () => {
    const queue = await renderSurface();
    type("running now");
    await clickSend();
    // The daemon's row for a session that has just taken a prompt says "working",
    // and the owner writes that reading onto the queue: it is the only thing a
    // steer defers to now (fix-4's single rule).
    pushActivity("working");
    type("jump in");
    await act(async () => pressEnter(true));
    await flush();
    vi.mocked(sessionSend).mockRejectedValueOnce(new Error("the session refused the steer"));

    // The row turns idle and the owner hands the queue that edge (fix-4).
    pushActivity("idle");
    await flush();

    expect(queuedTexts(queue)).toEqual(["jump in"]);
    const note = rows()[0].querySelector('[role="alert"]');
    expect(note?.textContent).toBe(SEND_FAILED);
    // One alert per failure (review F16): the row is where it is said, and the
    // composer line stays empty.
    expect(queueError()).toBeNull();
    expect(textarea().value).toBe("");
  });

  it("puts a refused plain send back into the composer, as Paseo does", async () => {
    await renderSurface();
    vi.mocked(sessionSend).mockRejectedValueOnce(new Error("Session input is too large."));

    type("too big");
    await act(async () => pressEnter());
    await flush();

    // Review F9: the composer cleared as soon as the send was called, and the
    // refusal left it empty with nothing back. The text is where the user left it.
    expect(textarea().value).toBe("too big");
    expect(document.activeElement).toBe(textarea());
  });

  it("under the steer setting Enter steers and Ctrl+Enter queues", async () => {
    setSendBehavior("interrupt-and-send");
    const queue = await renderSurface();
    type("running now");
    await clickSend();
    // The daemon's row for a session that has just taken a prompt says "working",
    // and the owner writes that reading onto the queue: it is the only thing a
    // steer defers to now (fix-4's single rule).
    pushActivity("working");

    type("from enter");
    await act(async () => pressEnter());
    await flush();
    expect(sessionInterrupt).toHaveBeenCalledTimes(1);
    // Ordered: the text waits for the turn-over rather than racing the cancel.
    expect(sessionSend).toHaveBeenCalledTimes(1);
    expect(queuedTexts(queue)).toEqual(["from enter"]);

    // The row turns idle and the owner hands the queue that edge (fix-4).
    pushActivity("idle");
    await flush();
    expect(sessionSend).toHaveBeenCalledTimes(2);
    expect(queuedTexts(queue)).toEqual([]);

    type("queued instead");
    await act(async () => pressEnter(true));
    await flush();
    expect(sessionSend).toHaveBeenCalledTimes(2);
    expect(queuedTexts(queue)).toEqual([]);
    expect(rows()).toHaveLength(0);
    expect(textarea().value).toBe("queued instead");
  });

  it("under the steer setting, Ctrl+Enter with no running turn does nothing, as Paseo's does", async () => {
    setSendBehavior("interrupt-and-send");
    const queue = await renderSurface();
    type("hold on");
    await act(async () => pressEnter(true));
    await flush();
    expect(sessionSend).not.toHaveBeenCalled();
    expect(sessionInterrupt).not.toHaveBeenCalled();
    expect(queuedTexts(queue)).toEqual([]);
    expect(textarea().value).toBe("hold on");
  });

  it("with a permission card open, Enter steers under the queue setting and the button label says so", async () => {
    const queue = await renderSurface();
    type("running now");
    await clickSend();
    // The daemon's row for a session that has just taken a prompt says "working",
    // and the owner writes that reading onto the queue: it is the only thing a
    // steer defers to now (fix-4's single rule).
    pushActivity("working");
    updateSurfaceProps({ hasPendingPermission: true });

    expect(container.querySelector('[data-testid="composer-queue-action"]')).toBeNull();

    type("behind a card");
    await act(async () => pressEnter());
    await flush();
    // Queueing here would strand the text behind a card nobody has answered,
    // so Enter becomes the steer: the interrupt goes out first.
    expect(sessionInterrupt).toHaveBeenCalledTimes(1);
    expect(queuedTexts(queue)).toEqual(["behind a card"]);

    // The row turns idle and the owner hands the queue that edge (fix-4).
    pushActivity("idle");
    await flush();
    expect(queuedTexts(queue)).toEqual([]);
    expect(sessionSend).toHaveBeenCalledTimes(2);
  });

  // Review F18: the properties the deleted steer-echo test used to pin — one
  // bubble per message, one answer, no turn split in two — now belong to the
  // path the app actually takes: a queued item sent by the drain, echoed by the
  // daemon, rendered once.
  it("renders a drained queue item once, with one answer and no split turn", async () => {
    const queue = await renderSurface();
    type("running now");
    await clickSend();
    // The daemon's row for a session that has just taken a prompt says "working",
    // and the owner writes that reading onto the queue: it is the only thing a
    // steer defers to now (fix-4's single rule).
    pushActivity("working");
    await act(async () => {
      harness.emit?.({
        type: "agent_user_message",
        author: "human",
        messageId: "user-1",
        messageKind: "composer",
        text: "running now",
      });
      harness.emit?.({ type: "agent_message", messageId: "answer-1", text: "the first answer" });
    });

    type("queued follow-up");
    await act(async () => pressEnter());
    await flush();
    expect(queuedTexts(queue)).toEqual(["queued follow-up"]);
    expect(sessionSend).toHaveBeenCalledTimes(1);

    // The row turns idle: the predicate falls, and that is what starts the drain.
    pushActivity("idle");
    await flush();
    expect(sessionSend).toHaveBeenCalledTimes(2);

    await act(async () => {
      harness.emit?.({
        type: "agent_user_message",
        author: "human",
        messageId: "user-2",
        messageKind: "composer",
        text: "queued follow-up",
      });
      harness.emit?.({ type: "agent_message", messageId: "answer-2", text: "the second answer" });
    });

    const copyIn = (selector: string): string[] =>
      [...container.querySelectorAll(selector)].map(
        (element) => element.querySelector(".workspace-chat-copy")?.textContent ?? "",
      );
    expect(copyIn(".workspace-chat-user")).toEqual(["running now", "queued follow-up"]);
    expect(copyIn(".workspace-chat-assistant")).toEqual(["the first answer", "the second answer"]);
    expect(queuedTexts(queue)).toEqual([]);
  });

  it("labels the composer action Queue message while the default queues", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    pushActivity("working");
    expect(queueAction().textContent).toBe("Queue message");
  });

  it("keeps the running-turn action button disabled while the composer is empty", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    pushActivity("working");
    expect(queueAction().disabled).toBe(true);
    type("   ");
    expect(queueAction().disabled).toBe(true);
    type("now it can queue");
    expect(queueAction().disabled).toBe(false);
  });

  it("keeps the composer's normal placeholder while the turn runs", async () => {
    await renderSurface();
    type("running now");
    await clickSend();
    expect(textarea().getAttribute("placeholder")).toBe(
      "Message the agent, or type / for commands",
    );
  });
});
